//! `halo proton scan`: searches the .exe and .dll of installed games for the fingerprints of the
//! lighting SDKs. It is the only reliable way to know whether a game drives lights: the public
//! lists are incomplete. A packed or encrypted game (Denuvo, etc.) may hide its strings, so
//! "nothing" means "nothing found", not "definitely not".

use crate::proton::{libraries, steam_roots, vdf_values};
use memchr::memmem::Finder;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub struct Sdk {
    pub name: &'static str,
    /// Whether Halo has a bridge for it.
    pub covered: bool,
    patterns: &'static [&'static str],
}

pub const SDKS: &[Sdk] = &[
    Sdk {
        name: "Razer Chroma (DLL)",
        covered: true,
        patterns: &["RzChromaSDK", "RzChromatic", "CChromaEditorLibrary", "ChromaAnimationAPI"],
    },
    Sdk { name: "Razer Chroma (REST)", covered: true, patterns: &["/razer/chromasdk"] },
    Sdk {
        name: "Logitech LED",
        covered: true,
        patterns: &[
            "LogitechLedEnginesWrapper",
            "LogiLedInit",
            "a6519e67-7632-4375-afdf-caa889744403",
            "A6519E67-7632-4375-AFDF-CAA889744403",
        ],
    },
    // The new (statically linked) wrapper runs the DLL from the key through WinVerifyTrust and
    // requires it to be signed by "Logitech Inc": ours is not, so in those games the bridge is useless.
    Sdk { name: "Logitech: requires signature", covered: false, patterns: &["Logitech Inc"] },
    Sdk {
        name: "SteelSeries GameSense",
        covered: true,
        patterns: &["coreProps.json", "bind_game_event", "SteelSeries Engine 3"],
    },
    Sdk { name: "Game State Integration", covered: false, patterns: &["gamestate_integration"] },
    Sdk { name: "Corsair iCUE", covered: false, patterns: &["CorsairSetLedsColors", "CUESDK.x64"] },
    Sdk { name: "ASUS Aura SDK", covered: false, patterns: &["aura.sdk.1", "AuraServiceLib"] },
    Sdk { name: "Alienware AlienFX", covered: false, patterns: &["LFX_Initialize"] },
];

/// Steam tools that live in `common` but are not games.
const TOOLS: &[&str] = &["Proton", "Steam Linux Runtime", "Steamworks Common", "Steamworks Shared"];

const CHUNK: usize = 4 << 20;

pub struct Game {
    pub appid: String,
    pub name: String,
    pub dir: PathBuf,
    /// `StateFlags` 4 = fully installed.
    pub complete: bool,
}

pub struct Hit {
    pub sdk: &'static Sdk,
    pub file: PathBuf,
    pub pattern: &'static str,
}

pub fn installed_games() -> Vec<Game> {
    let mut games = Vec::new();
    for lib in libraries(&steam_roots()) {
        let Ok(entries) = fs::read_dir(lib.join("steamapps")) else { continue };
        for e in entries.flatten() {
            let file = e.file_name().to_string_lossy().into_owned();
            let Some(appid) = file.strip_prefix("appmanifest_").and_then(|f| f.strip_suffix(".acf")) else { continue };
            let Ok(acf) = fs::read_to_string(e.path()) else { continue };
            let first = |k| vdf_values(&acf, k).into_iter().next();
            let (Some(name), Some(installdir)) = (first("name"), first("installdir")) else { continue };
            if TOOLS.iter().any(|t| name.starts_with(t)) {
                continue;
            }
            let complete = first("StateFlags").as_deref() == Some("4");
            games.push(Game { appid: appid.into(), name, dir: lib.join("steamapps/common").join(installdir), complete });
        }
    }
    games.sort_by_key(|g| g.name.to_lowercase());
    games
}

/// The .exe and .dll under `dir`, without following links.
fn binaries(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        let path = e.path();
        if ft.is_dir() {
            binaries(&path, out);
        } else if ft.is_file()
            && path.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe") || x.eq_ignore_ascii_case("dll"))
        {
            out.push(path);
        }
    }
}

struct Needle {
    sdk: usize,
    pattern: &'static str,
    finder: Finder<'static>,
}

/// Each pattern in ASCII and in UTF-16LE (Windows strings and .NET's #US heap).
fn needles() -> Vec<Needle> {
    let mut out = Vec::new();
    for (sdk, s) in SDKS.iter().enumerate() {
        for &pattern in s.patterns {
            let wide: Vec<u8> = pattern.encode_utf16().flat_map(u16::to_le_bytes).collect();
            for bytes in [pattern.as_bytes().to_vec(), wide] {
                out.push(Needle { sdk, pattern, finder: Finder::new(&bytes).into_owned() });
            }
        }
    }
    out
}

/// Walks the file in blocks (with overlap so a pattern is not split) and records the SDKs that
/// `found` did not have yet.
fn scan_file(path: &Path, needles: &[Needle], found: &mut [Option<Hit>]) -> io::Result<()> {
    let overlap = needles.iter().map(|n| n.finder.needle().len()).max().unwrap_or(0);
    let mut f = File::open(path)?;
    let mut buf = vec![0u8; CHUNK + overlap];
    let mut kept = 0;
    loop {
        let n = f.read(&mut buf[kept..])?;
        if n == 0 {
            return Ok(());
        }
        let len = kept + n;
        for nd in needles {
            if found[nd.sdk].is_none() && nd.finder.find(&buf[..len]).is_some() {
                found[nd.sdk] = Some(Hit { sdk: &SDKS[nd.sdk], file: path.to_path_buf(), pattern: nd.pattern });
            }
        }
        if found.iter().all(Option::is_some) {
            return Ok(());
        }
        kept = overlap.min(len);
        buf.copy_within(len - kept..len, 0);
    }
}

/// What was found in the game, in the order of `SDKS`.
pub fn scan(game: &Game) -> (Vec<Hit>, usize) {
    let mut files = Vec::new();
    binaries(&game.dir, &mut files);
    let needles = needles();
    let mut found: Vec<Option<Hit>> = SDKS.iter().map(|_| None).collect();
    for f in &files {
        let _ = scan_file(f, &needles, &mut found);
    }
    (found.into_iter().flatten().collect(), files.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_ascii_wide_and_across_chunks() {
        let dir = std::env::temp_dir().join(format!("halo-scan-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Binaries/Win64")).unwrap();
        // Logitech split right at a block boundary.
        let mut exe = vec![0u8; CHUNK - 5];
        exe.extend_from_slice(b"LogiLedInit");
        fs::write(dir.join("Binaries/Win64/Game.exe"), exe).unwrap();
        let wide: Vec<u8> = "RzChromaSDK64.dll".encode_utf16().flat_map(u16::to_le_bytes).collect();
        fs::write(dir.join("Plugin.DLL"), wide).unwrap();
        fs::write(dir.join("notes.txt"), b"bind_game_event").unwrap();

        let game = Game { appid: "1".into(), name: "x".into(), dir: dir.clone(), complete: true };
        let (hits, files) = scan(&game);
        assert_eq!(files, 2);
        let names: Vec<_> = hits.iter().map(|h| h.sdk.name).collect();
        assert_eq!(names, ["Razer Chroma (DLL)", "Logitech LED"]);
        fs::remove_dir_all(&dir).unwrap();
    }
}
