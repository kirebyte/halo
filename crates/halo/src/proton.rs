//! `halo proton`: installs or removes Halo's bridges in Proton prefixes (compatdata):
//! - the Chroma DLL (`RzChromaSDK*`/`RzChromatic*`);
//! - `LogitechLed.dll` and the registry key through which each game's Logitech wrapper finds it;
//! - the `coreProps.json` through which SteelSeries GameSense games find the bridge.
//!
//! The DLLs live in `~/.local/share/halo/{chroma,logitech}/{x64,x86}` (install.sh copies them).
//! Removal only deletes files carrying Halo's mark and keys pointing at our DLL: never anything
//! from Razer or Logitech.

use halo_core::tr;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// (build dir, prefix dir, names)
const TARGETS: [(&str, &str, [&str; 2]); 2] = [
    ("chroma/x64", "system32", ["RzChromaSDK64.dll", "RzChromatic64.dll"]),
    ("chroma/x86", "syswow64", ["RzChromaSDK.dll", "RzChromatic.dll"]),
];
/// Found in the VERSIONINFO of `shims/chroma/version.rc` (UTF-16).
const MARK: &str = "Halo: Razer Chroma bridge";

const LOGITECH_DLL: &str = "LogitechLed.dll";
/// (build dir, prefix dir, registry key for that architecture, as in regedit)
const LOGITECH: [(&str, &str, &str); 2] = [
    ("logitech/x64", "system32", r"Software\Classes\CLSID\{a6519e67-7632-4375-afdf-caa889744403}\ServerBinary"),
    ("logitech/x86", "syswow64", r"Software\Classes\Wow6432Node\CLSID\{a6519e67-7632-4375-afdf-caa889744403}\ServerBinary"),
];
/// Found in the VERSIONINFO of `shims/logitech/version.rc`.
const LOGITECH_MARK: &str = "Halo: Logitech LED bridge";

/// Where SteelSeries Engine leaves its address (`%PROGRAMDATA%`), relative to `drive_c`.
const CORE_PROPS: &str = "ProgramData/SteelSeries/SteelSeries Engine 3/coreProps.json";

fn core_props(windows: &Path) -> PathBuf {
    windows.parent().unwrap_or(windows).join(CORE_PROPS)
}

/// Ours carries `"halo": true`; one from a real SteelSeries Engine is left alone.
fn core_props_ours(path: &Path) -> bool {
    fs::read(path).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()).is_some_and(|v| v["halo"] == true)
}

pub struct Prefix {
    pub appid: String,
    pub name: Option<String>,
    pub windows: PathBuf,
}

pub fn dll_dir() -> PathBuf {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"));
    data.join("halo")
}

pub(crate) fn steam_roots() -> Vec<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    [".local/share/Steam", ".steam/steam", ".var/app/com.valvesoftware.Steam/data/Steam"]
        .iter()
        .map(|p| home.join(p))
        .filter(|p| p.is_dir())
        .collect()
}

/// The Steam libraries: the main one plus those in `libraryfolders.vdf`.
pub fn libraries(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut libs = Vec::new();
    for root in roots {
        libs.push(root.clone());
        if let Ok(vdf) = fs::read_to_string(root.join("steamapps/libraryfolders.vdf")) {
            libs.extend(vdf_values(&vdf, "path").into_iter().map(PathBuf::from));
        }
    }
    // By inode, not by path: inside a container the same directory can be mounted twice under
    // different paths, with no symlink for canonicalize to resolve.
    let mut seen = Vec::new();
    libs.retain(|l| {
        let Ok(meta) = fs::metadata(l) else { return false };
        let key = (meta.dev(), meta.ino());
        !seen.contains(&key) && {
            seen.push(key);
            true
        }
    });
    libs
}

/// The values of `"key"  "value"` in a Steam VDF file (no nesting: enough for path and name).
pub(crate) fn vdf_values(text: &str, key: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| {
            let mut parts = l.split('"').skip(1).step_by(2);
            (parts.next()? == key).then(|| parts.next().map(|v| v.replace("\\\\", "\\")))?
        })
        .collect()
}

pub fn prefixes(libs: &[PathBuf]) -> Vec<Prefix> {
    let mut out = Vec::new();
    for lib in libs {
        let Ok(entries) = fs::read_dir(lib.join("steamapps/compatdata")) else { continue };
        for e in entries.flatten() {
            let appid = e.file_name().to_string_lossy().into_owned();
            let windows = e.path().join("pfx/drive_c/windows");
            if !windows.join("system32").is_dir() {
                continue;
            }
            let name = libs.iter().find_map(|l| {
                let acf = fs::read_to_string(l.join(format!("steamapps/appmanifest_{appid}.acf"))).ok()?;
                vdf_values(&acf, "name").into_iter().next()
            });
            // Proton and the Steam Linux Runtime also have a prefix, but they are not games.
            if name.as_deref().is_some_and(|n| n.starts_with("Proton ") || n.starts_with("Steam Linux Runtime")) {
                continue;
            }
            out.push(Prefix { appid, name, windows });
        }
    }
    out.sort_by(|a, b| a.appid.cmp(&b.appid));
    out
}

pub fn find_prefixes() -> Vec<Prefix> {
    prefixes(&libraries(&steam_roots()))
}

fn has_mark(path: &Path, mark: &str) -> bool {
    let mark: Vec<u8> = mark.encode_utf16().flat_map(u16::to_le_bytes).collect();
    fs::read(path).is_ok_and(|b| b.windows(mark.len()).any(|w| w == mark))
}

fn is_ours(path: &Path) -> bool {
    has_mark(path, MARK)
}

fn is_our_logitech(path: &Path) -> bool {
    has_mark(path, LOGITECH_MARK)
}

// ---- Wine's system.reg ----
// Text with one section per key: `[Software\\Classes\\…] <time>` and its values (`@="…"` is the
// default value), up to the next section. Wine rewrites it when the prefix closes, so this is
// done with the game closed.

fn reg_sections(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, _) in text.match_indices("\n[") {
        out.push(&text[start..=i]);
        start = i + 1;
    }
    out.push(&text[start..]);
    out
}

/// In `system.reg` the `\` in key names are doubled.
fn reg_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
}

fn reg_is(section: &str, key: &str) -> bool {
    section.to_ascii_lowercase().starts_with(&format!("[{}]", reg_escape(key).to_ascii_lowercase()))
}

/// The default value of a key, unescaped.
fn reg_default(text: &str, key: &str) -> Option<String> {
    let section = reg_sections(text).into_iter().find(|s| reg_is(s, key))?;
    let line = section.lines().find(|l| l.starts_with("@=\""))?;
    Some(line[3..].strip_suffix('"')?.replace("\\\\", "\\"))
}

fn reg_without(text: &str, key: &str) -> String {
    let sections = reg_sections(text);
    let kept: Vec<&str> = sections.iter().copied().filter(|s| !reg_is(s, key)).collect();
    let mut out: String = kept.concat();
    if kept.len() < sections.len() && out.ends_with("\n\n") {
        // It was the last section: the blank line that separated it goes too.
        out.truncate(out.trim_end_matches('\n').len() + 1);
    }
    out
}

fn reg_with(text: &str, key: &str, value: &str) -> String {
    let mut out = reg_without(text, key);
    if !out.ends_with("\n\n") {
        out.push_str(if out.ends_with('\n') { "\n" } else { "\n\n" });
    }
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    out.push_str(&format!("[{}] {secs}\n@=\"{}\"\n", reg_escape(key), reg_escape(value)));
    out
}

/// `drive_c/windows` → `pfx/system.reg`.
fn system_reg(windows: &Path) -> PathBuf {
    windows.parent().and_then(Path::parent).unwrap_or(windows).join("system.reg")
}

fn logitech_value(dir: &str) -> String {
    format!(r"C:\windows\{dir}\{LOGITECH_DLL}")
}

/// Copies the Logitech DLL and registers it so the games' wrapper loads it. A key that already
/// points at another DLL (Logitech G HUB, Artemis…) is left alone.
fn install_logitech(windows: &Path, dlls: &Path) -> io::Result<()> {
    let reg_path = system_reg(windows);
    let mut reg = fs::read_to_string(&reg_path)?;
    for (src, dir, key) in LOGITECH {
        let dest = windows.join(dir);
        if !dest.is_dir() {
            continue;
        }
        let file = dest.join(LOGITECH_DLL);
        if file.exists() && !is_our_logitech(&file) {
            return Err(io::Error::other(tr!("There is already a {file} in {dir} that is not Halo's; leaving it alone", file = LOGITECH_DLL, dir = dir)));
        }
        let src = dlls.join(src).join(LOGITECH_DLL);
        if !src.exists() {
            return Err(io::Error::new(io::ErrorKind::NotFound, tr!("Missing {path} (run install.sh)", path = src.display())));
        }
        fs::copy(&src, &file)?;
        let value = logitech_value(dir);
        match reg_default(&reg, key) {
            Some(v) if !v.eq_ignore_ascii_case(&value) => eprintln!("warning: the registry already points to {v}; leaving it alone"),
            _ => reg = reg_with(&reg, key, &value),
        }
    }
    fs::write(&reg_path, reg)
}

fn remove_logitech(windows: &Path) -> io::Result<usize> {
    let mut n = 0;
    let reg_path = system_reg(windows);
    if let Ok(mut reg) = fs::read_to_string(&reg_path) {
        let before = reg.len();
        for (_, dir, key) in LOGITECH {
            if reg_default(&reg, key).is_some_and(|v| v.eq_ignore_ascii_case(&logitech_value(dir))) {
                reg = reg_without(&reg, key);
                n += 1;
            }
        }
        if reg.len() != before {
            fs::write(&reg_path, reg)?;
        }
    }
    for (_, dir, _) in LOGITECH {
        let file = windows.join(dir).join(LOGITECH_DLL);
        if file.exists() && is_our_logitech(&file) {
            fs::remove_file(file)?;
            n += 1;
        }
    }
    Ok(n)
}

#[derive(Debug, PartialEq, Eq)]
pub enum Status {
    Installed,
    Missing,
    /// There is a Chroma DLL that is not Halo's (e.g. Razer's): it is left alone.
    Foreign,
}

pub fn status(windows: &Path) -> Status {
    let files: Vec<PathBuf> = TARGETS.iter().flat_map(|(_, dir, names)| names.iter().map(move |n| windows.join(dir).join(n))).collect();
    if files.iter().any(|f| f.exists() && !is_ours(f)) {
        Status::Foreign
    } else if files.iter().all(|f| f.exists() || !f.parent().is_some_and(Path::is_dir)) {
        Status::Installed
    } else {
        Status::Missing
    }
}

pub fn install(windows: &Path, dlls: &Path) -> io::Result<()> {
    if status(windows) == Status::Foreign {
        return Err(io::Error::other(tr!("There is already a Chroma DLL that is not Halo's; leaving it alone")));
    }
    for (arch, dir, names) in TARGETS {
        let dest = windows.join(dir);
        if !dest.is_dir() {
            continue; // prefix without 32-bit
        }
        for name in names {
            let src = dlls.join(arch).join(name);
            if !src.exists() {
                return Err(io::Error::new(io::ErrorKind::NotFound, tr!("Missing {path} (run install.sh)", path = src.display())));
            }
            fs::copy(&src, dest.join(name))?;
        }
    }
    install_logitech(windows, dlls)?;
    let props = core_props(windows);
    if !props.exists() || core_props_ours(&props) {
        let port = crate::daemon::bridges::gamesense::PORT;
        fs::create_dir_all(props.parent().unwrap())?;
        fs::write(&props, serde_json::json!({ "address": format!("127.0.0.1:{port}"), "halo": true }).to_string())?;
    }
    Ok(())
}

/// Removes only Halo's files. Returns how many files and keys it removed.
pub fn remove(windows: &Path) -> io::Result<usize> {
    let mut n = remove_logitech(windows)?;
    for (_, dir, names) in TARGETS {
        for name in names {
            let f = windows.join(dir).join(name);
            if f.exists() && is_ours(&f) {
                fs::remove_file(f)?;
                n += 1;
            }
        }
    }
    let props = core_props(windows);
    if core_props_ours(&props) {
        fs::remove_file(props)?;
        n += 1;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("halo-proton-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn marked(mark: &str) -> Vec<u8> {
        let mark: Vec<u8> = mark.encode_utf16().flat_map(u16::to_le_bytes).collect();
        [b"MZ".as_slice(), &mark].concat()
    }

    fn fake_dlls(root: &Path) -> PathBuf {
        let dlls = root.join("dlls");
        for (arch, _, names) in TARGETS {
            fs::create_dir_all(dlls.join(arch)).unwrap();
            for n in names {
                fs::write(dlls.join(arch).join(n), marked(MARK)).unwrap();
            }
        }
        for (arch, _, _) in LOGITECH {
            fs::create_dir_all(dlls.join(arch)).unwrap();
            fs::write(dlls.join(arch).join(LOGITECH_DLL), marked(LOGITECH_MARK)).unwrap();
        }
        dlls
    }

    /// A minimal prefix: `pfx/drive_c/windows/{system32,syswow64}` and a `system.reg` with another key.
    fn fake_prefix(root: &Path) -> PathBuf {
        let win = root.join("pfx/drive_c/windows");
        fs::create_dir_all(win.join("system32")).unwrap();
        fs::create_dir_all(win.join("syswow64")).unwrap();
        fs::write(root.join("pfx/system.reg"), REG).unwrap();
        win
    }

    const REG: &str = "WINE REGISTRY Version 2\n;; All keys relative to \\\\Machine\n\n#arch=win64\n\n[Software\\\\Wine] 1700000000\n#time=1\n\"Version\"=\"win10\"\n";

    #[test]
    fn finds_prefixes_and_names() {
        let root = tmp("find");
        let extra = root.join("SSD/Steam");
        fs::create_dir_all(root.join("Steam/steamapps")).unwrap();
        fs::write(
            root.join("Steam/steamapps/libraryfolders.vdf"),
            format!("\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n", extra.display()),
        )
        .unwrap();
        fs::create_dir_all(extra.join("steamapps/compatdata/1091500/pfx/drive_c/windows/system32")).unwrap();
        fs::write(extra.join("steamapps/appmanifest_1091500.acf"), "\"AppState\"\n{\n\t\"appid\"\t\t\"1091500\"\n\t\"name\"\t\t\"A Game\"\n}\n").unwrap();
        let found = prefixes(&libraries(&[root.join("Steam")]));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].appid, "1091500");
        assert_eq!(found[0].name.as_deref(), Some("A Game"));
    }

    #[test]
    fn install_remove_and_foreign() {
        let root = tmp("install");
        let dlls = fake_dlls(&root);
        let win = fake_prefix(&root);
        assert_eq!(status(&win), Status::Missing);
        install(&win, &dlls).unwrap();
        assert_eq!(status(&win), Status::Installed);
        let props = win.parent().unwrap().join(CORE_PROPS);
        let text = fs::read_to_string(&props).unwrap();
        assert!(text.contains("127.0.0.1:54237"), "{text}");
        assert!(win.join("syswow64/LogitechLed.dll").exists());
        // 4 Chroma DLLs, 2 Logitech, 2 keys and the coreProps.json.
        assert_eq!(remove(&win).unwrap(), 9);
        assert_eq!(status(&win), Status::Missing);
        assert!(!props.exists());

        // A real SteelSeries Engine coreProps.json stays.
        fs::write(&props, r#"{"address":"127.0.0.1:51248"}"#).unwrap();
        install(&win, &dlls).unwrap();
        assert_eq!(fs::read_to_string(&props).unwrap(), r#"{"address":"127.0.0.1:51248"}"#);
        assert_eq!(remove(&win).unwrap(), 8);
        assert!(props.exists());

        fs::write(win.join("system32/RzChromaSDK64.dll"), b"MZ from Razer").unwrap();
        assert_eq!(status(&win), Status::Foreign);
        assert!(install(&win, &dlls).is_err());
        assert_eq!(remove(&win).unwrap(), 0);
        assert!(win.join("system32/RzChromaSDK64.dll").exists());
    }

    #[test]
    fn logitech_registry() {
        let root = tmp("logitech");
        let dlls = fake_dlls(&root);
        let win = fake_prefix(&root);
        let reg_path = root.join("pfx/system.reg");
        install(&win, &dlls).unwrap();
        let reg = fs::read_to_string(&reg_path).unwrap();
        assert!(reg.starts_with(REG), "what was there stays the same");
        // Wine's exact format: doubled backslashes in the key name and in the value.
        assert!(reg.contains(r"[Software\\Classes\\CLSID\\{a6519e67-7632-4375-afdf-caa889744403}\\ServerBinary] "), "{reg}");
        assert_eq!(reg_default(&reg, LOGITECH[0].2).as_deref(), Some(r"C:\windows\system32\LogitechLed.dll"));
        assert!(reg.contains(r#"@="C:\\windows\\syswow64\\LogitechLed.dll""#), "{reg}");
        install(&win, &dlls).unwrap();
        assert_eq!(fs::read_to_string(&reg_path).unwrap().matches("LogitechLed.dll").count(), 2, "installing twice does not duplicate");
        remove(&win).unwrap();
        assert_eq!(fs::read_to_string(&reg_path).unwrap(), REG, "removing leaves the registry as it was");

        // A G HUB (or other) key is left alone, both on install and on removal.
        let ghub = reg_with(REG, LOGITECH[0].2, r"C:\Program Files\LGHUB\sdk_legacy_led_x64.dll");
        fs::write(&reg_path, &ghub).unwrap();
        install(&win, &dlls).unwrap();
        let reg = fs::read_to_string(&reg_path).unwrap();
        assert_eq!(reg_default(&reg, LOGITECH[0].2).as_deref(), Some(r"C:\Program Files\LGHUB\sdk_legacy_led_x64.dll"));
        remove(&win).unwrap();
        assert!(fs::read_to_string(&reg_path).unwrap().contains("LGHUB"), "the G HUB key stays");
    }
}
