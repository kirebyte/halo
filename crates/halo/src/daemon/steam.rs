//! Which Steam game is running, without polling loops (for per-game profiles).
//!
//! Steam does not announce it over D-Bus nor leave the current game in any file (`RunningAppID`
//! in `registry.vdf` is no longer written; verified with Factorio). What it does do is rewrite
//! `steamapps/appmanifest_APPID.acf` with `LastPlayed = now` when a game is **launched** and
//! when it is **closed**. So:
//! - inotify on every `steamapps/`: a manifest with a recent `LastPlayed` says "something
//!   happened with that AppID";
//! - its `reaper SteamLaunch AppId=N` is looked up in `/proc` (one pass, only then): if present,
//!   the game just started;
//! - a `pidfd` of the reaper wakes up when the game exits. Zero polling.
//!
//! On daemon startup one pass is made in case a game is already open.

use crate::proton;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::Interest;
use tokio::io::unix::AsyncFd;
use tokio::sync::mpsc::{self, Sender};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Game {
    pub appid: String,
    pub name: String,
    pub pid: i32,
}

/// How far `LastPlayed` may be from now to count as "just happened".
const FRESH: u64 = 20;
/// Steam writes the manifest almost at the same time it launches the reaper: give it a few seconds.
const LOOK_AGAIN: [Duration; 3] = [Duration::ZERO, Duration::from_secs(1), Duration::from_secs(3)];

enum Event {
    /// A freshly written manifest with `LastPlayed` set to now.
    Manifest(String),
    Exited(i32),
}

/// `reaper SteamLaunch AppId=427520 -- ...` → `427520`.
pub fn parse_reaper(cmdline: &[u8]) -> Option<String> {
    let mut args = cmdline.split(|b| *b == 0);
    let exe = args.next()?;
    if !exe.ends_with(b"/reaper") || args.next()? != b"SteamLaunch" {
        return None;
    }
    let appid = args.next()?.strip_prefix(b"AppId=")?;
    let appid = std::str::from_utf8(appid).ok()?;
    (!appid.is_empty() && appid.bytes().all(|b| b.is_ascii_digit())).then(|| appid.to_string())
}

/// Steam games running now: (reaper pid, AppID). One pass over `/proc`.
pub fn reapers() -> Vec<(i32, String)> {
    let mut out = Vec::new();
    for e in fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) else { continue };
        if let Ok(cmd) = fs::read(e.path().join("cmdline"))
            && let Some(appid) = parse_reaper(&cmd)
        {
            out.push((pid, appid));
        }
    }
    out.sort();
    out
}

/// `appmanifest_427520.acf` → `427520`.
pub fn manifest_appid(name: &str) -> Option<&str> {
    let appid = name.strip_prefix("appmanifest_")?.strip_suffix(".acf")?;
    (!appid.is_empty() && appid.bytes().all(|b| b.is_ascii_digit())).then_some(appid)
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

pub fn fresh(last_played: u64, now: u64) -> bool {
    last_played.abs_diff(now) <= FRESH
}

fn manifest(libs: &[PathBuf], appid: &str) -> Option<String> {
    libs.iter().find_map(|l| fs::read_to_string(l.join(format!("steamapps/appmanifest_{appid}.acf"))).ok())
}

/// The game's name according to its manifest, or the AppID if it is not installed.
pub fn game_name(libs: &[PathBuf], appid: &str) -> String {
    manifest(libs, appid).and_then(|m| proton::vdf_values(&m, "name").into_iter().next()).unwrap_or_else(|| appid.to_string())
}

/// Installed games: (AppID, name), sorted by name.
pub fn installed(libs: &[PathBuf]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for lib in libs {
        for e in fs::read_dir(lib.join("steamapps")).into_iter().flatten().flatten() {
            let file = e.file_name();
            let Some(appid) = file.to_str().and_then(manifest_appid) else { continue };
            if let Ok(m) = fs::read_to_string(e.path())
                && let Some(name) = proton::vdf_values(&m, "name").into_iter().next()
            {
                out.push((appid.to_string(), name));
            }
        }
    }
    out.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()).then_with(|| a.0.cmp(&b.0)));
    out.dedup();
    out
}

pub fn libraries() -> Vec<PathBuf> {
    proton::libraries(&proton::steam_roots())
}

/// Wakes up when the process exits (pidfd + epoll). No loop.
async fn wait_exit(pid: i32) -> io::Result<()> {
    // SAFETY: pidfd_open with flags 0 touches none of our memory.
    let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the descriptor is ours and nobody else closes it.
    let fd = unsafe { OwnedFd::from_raw_fd(raw as i32) };
    // SAFETY: `fd` is an open OwnedFd that the AsyncFd owns until dropped.
    let fd = unsafe { AsyncFd::register_with_interest(fd, Interest::READABLE) }.map_err(io::Error::from)?;
    let guard = fd.readable().await?;
    drop(guard);
    Ok(())
}

fn inotify(libs: &[PathBuf]) -> io::Result<(AsyncFd<File>, HashMap<i32, PathBuf>)> {
    // SAFETY: libc calls without pointers of ours, except the NUL-terminated path.
    let raw = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: just created and ours.
    let file = File::from(unsafe { OwnedFd::from_raw_fd(raw) });
    let mut wds = HashMap::new();
    for lib in libs {
        let dir = lib.join("steamapps");
        let c = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes()).map_err(|_| io::Error::other("path with a NUL byte"))?;
        // SAFETY: `c` lives for the duration of the call.
        let wd = unsafe { libc::inotify_add_watch(file.as_raw_fd(), c.as_ptr(), libc::IN_CLOSE_WRITE | libc::IN_MOVED_TO) };
        if wd < 0 {
            eprintln!("halo: could not watch {} ({})", dir.display(), io::Error::last_os_error());
            continue;
        }
        wds.insert(wd, dir);
    }
    if wds.is_empty() {
        return Err(io::Error::other("no steamapps folder to watch"));
    }
    // SAFETY: `file` is an open File that the AsyncFd owns until dropped.
    let fd = unsafe { AsyncFd::register_with_interest(file, Interest::READABLE) }.map_err(io::Error::from)?;
    Ok((fd, wds))
}

/// Reads inotify events and sends the manifests whose `LastPlayed` is now.
async fn manifests(fd: AsyncFd<File>, wds: HashMap<i32, PathBuf>, tx: Sender<Event>) -> io::Result<()> {
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let mut guard = fd.readable().await?;
        let n = match guard.try_io(|f| (&*f.get_ref()).read(&mut buf)) {
            Ok(r) => r?,
            Err(_would_block) => continue,
        };
        let mut i = 0;
        while i + 16 <= n {
            let wd = i32::from_ne_bytes(buf[i..i + 4].try_into().expect("4 bytes"));
            let len = u32::from_ne_bytes(buf[i + 12..i + 16].try_into().expect("4 bytes")) as usize;
            let name = &buf[i + 16..(i + 16 + len).min(n)];
            i += 16 + len;
            let name = std::str::from_utf8(name.split(|b| *b == 0).next().unwrap_or(&[])).unwrap_or("");
            let Some(appid) = manifest_appid(name) else { continue };
            let Some(dir) = wds.get(&wd) else { continue };
            let Ok(acf) = fs::read_to_string(dir.join(name)) else { continue };
            let played = proton::vdf_values(&acf, "LastPlayed").into_iter().next().and_then(|v| v.parse::<u64>().ok());
            if played.is_some_and(|p| fresh(p, now())) && tx.send(Event::Manifest(appid.to_string())).await.is_err() {
                return Ok(());
            }
        }
    }
}

/// Sends the current game whenever it changes (`None` when the last one closes). With several
/// open, the most recent.
pub async fn watch(out: Sender<Option<Game>>) {
    let libs = libraries();
    if libs.is_empty() {
        eprintln!("halo: Steam not found; no per-game profiles");
        return;
    }
    let (tx, mut rx) = mpsc::channel::<Event>(16);
    let (fd, wds) = match inotify(&libs) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("halo: no per-game profiles: could not watch steamapps ({e})");
            return;
        }
    };
    tokio::spawn(manifests(fd, wds, tx.clone()));
    eprintln!("halo: following Steam games in {} libraries", libs.len());

    let mut running: Vec<Game> = Vec::new();
    let track = |running: &mut Vec<Game>, pid: i32, appid: String| {
        let name = game_name(&libs, &appid);
        eprintln!("halo: game opened: {name} ({appid})");
        running.push(Game { appid, name, pid });
        let tx = tx.clone();
        tokio::spawn(async move {
            if let Err(e) = wait_exit(pid).await {
                eprintln!("halo: could not wait for game {pid} ({e})");
            }
            let _ = tx.send(Event::Exited(pid)).await;
        });
    };
    for (pid, appid) in reapers() {
        track(&mut running, pid, appid);
    }
    if let Some(g) = running.last() && out.send(Some(g.clone())).await.is_err() {
        return;
    }
    while let Some(ev) = rx.recv().await {
        match ev {
            Event::Manifest(appid) => {
                if running.iter().any(|g| g.appid == appid) {
                    continue; // written on close; the pidfd handles it
                }
                for wait in LOOK_AGAIN {
                    tokio::time::sleep(wait).await;
                    if let Some((pid, _)) = reapers().into_iter().find(|(_, a)| *a == appid) {
                        track(&mut running, pid, appid);
                        break;
                    }
                }
            }
            Event::Exited(pid) => {
                let Some(i) = running.iter().position(|g| g.pid == pid) else { continue };
                let g = running.remove(i);
                eprintln!("halo: game closed: {} ({})", g.name, g.appid);
            }
        }
        if out.send(running.last().cloned()).await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reaper_cmdline() {
        let cmd = b"/home/k/.local/share/Steam/ubuntu12_32/reaper\0SteamLaunch\0AppId=427520\0--\0/x/proton\0";
        assert_eq!(parse_reaper(cmd), Some("427520".into()));
        assert_eq!(parse_reaper(b"/usr/bin/reaper\0SteamLaunch\0AppId=\0"), None);
        assert_eq!(parse_reaper(b"/usr/bin/bash\0SteamLaunch\0AppId=1\0"), None);
        assert_eq!(parse_reaper(b"/usr/bin/reaper\0Other\0AppId=1\0"), None);
        assert_eq!(parse_reaper(b""), None);
    }

    #[test]
    fn manifest_names_and_freshness() {
        assert_eq!(manifest_appid("appmanifest_427520.acf"), Some("427520"));
        assert_eq!(manifest_appid("appmanifest_427520.acf.tmp"), None);
        assert_eq!(manifest_appid("libraryfolders.vdf"), None);
        assert_eq!(manifest_appid("appmanifest_.acf"), None);
        assert!(fresh(1000, 1010));
        assert!(fresh(1010, 1000), "Steam's clock may run slightly ahead");
        assert!(!fresh(1000, 1000 + FRESH + 1));
    }

    #[test]
    fn installed_and_names() {
        let root = std::env::temp_dir().join(format!("halo-steam-{}", std::process::id()));
        let apps = root.join("steamapps");
        fs::create_dir_all(&apps).unwrap();
        fs::write(apps.join("appmanifest_2.acf"), "\"AppState\"\n{\n\t\"appid\"\t\t\"2\"\n\t\"name\"\t\t\"Zeta\"\n\t\"LastPlayed\"\t\t\"5\"\n}\n").unwrap();
        fs::write(apps.join("appmanifest_1.acf"), "\"AppState\"\n{\n\t\"name\"\t\t\"alpha\"\n}\n").unwrap();
        fs::write(apps.join("appmanifest_x.acf"), "\"name\"\t\"no\"").unwrap();
        let libs = vec![root.clone()];
        assert_eq!(installed(&libs), vec![("1".to_string(), "alpha".to_string()), ("2".to_string(), "Zeta".to_string())]);
        assert_eq!(game_name(&libs, "2"), "Zeta");
        assert_eq!(game_name(&libs, "9"), "9");
        fs::remove_dir_all(root).unwrap();
    }
}
