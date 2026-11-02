# The window (`halo-gui`)

Qt 6 + Kirigami, written in Rust with cxx-qt (`crates/halo-gui`). It never touches the hardware:
everything goes through the daemon (`org.halo.Halo1` on the session bus). If the daemon is not
there, it says so at the bottom and waits.

## Pages
- **Lights:** the "Rings off" switch (Armoury Crate's "Dark": hardware brightness 0, nothing turns
  them on until it is switched off; then the last brightness returns), the two rings with the real
  color of each half (zones 0 left bottom, 1 left top, 2 right top, 3 right bottom). Tapping a half
  opens the color dialog for that zone; "One color for all four" sets them all. Hardware brightness
  (0 = off), effect and speed. With an effect on, the "Effect colors and direction" card edits its
  custom colors (tap a circle to change it; Add, Remove the last, Defaults) and the "Reverse"
  switch for rainbow, cycle, comet and tide; the same as `halo effect colors|direction`. Active
  layers (games, OpenRGB) are listed at the bottom.
- **Games:** the global permission and, per game, a switch to block or allow it (the names are
  those the games give the bridges; see `bridges.md`). Below, the running Steam game (with what to
  give it when it opens: the current color, off or an effect) and the list of profiles, with adding
  one from the installed games (see `profiles.md`). At the end, the Proton prefixes with a switch
  per game to put or remove the bridge (the same as `halo proton install|remove`; over D-Bus
  `Prefixes` and `SetBridgeInstalled`). A prefix with a foreign Chroma DLL shows disabled: it is
  left alone.
- **Settings:** off with the screen, hardware effects, latitude and longitude for `sun`.

All text goes through `Halo.tr(...)` and follows the session language (en, es, fr, ja).

## How it reads the state
A QML `Timer` calls `Halo.refresh()` every second (16 D-Bus calls, about 1 ms). With a software
effect on it drops to 200 ms so the rings in the window move smoothly. Effects animated by the MCU
do not move in the window: the daemon does not know what the MCU is painting.

`Halo` is a singleton of the QML module `org.halo.gui` (`src/halo.rs`): read-only properties
(`frame`, `base`, `effect`, `gamesJson`, `configJson`, ...) and actions (`setZones`,
`applyBrightness`, `startEffect`, `setGameAllowed`, `setConfig`, ...). Each action calls the daemon
and refreshes; the error, if any, shows in the footer's `InlineMessage`. zbus 6 is async, so
`src/daemon.rs` carries a single-thread tokio runtime and blocks on each call.

## Build and run
```bash
QMAKE=/usr/bin/qmake6 cargo build -p halo-gui   # needs qt6-base, qt6-declarative, qt6-svg, kirigami, kirigami-addons, qqc2-breeze-style, breeze-icons, cmake
target/debug/halo-gui
```
`halo-gui` is not in the workspace's `default-members`: a plain `cargo build` skips it, since it
needs Qt. `install.sh` builds it when it finds `qmake6`, puts it in `~/.local/share/halo/bin/halo-gui`
and adds a launcher `~/.local/bin/halo-gui`; inside a distrobox the launcher enters the container,
because the binary uses the container's Qt. The `.desktop` (`packaging/desktop/org.halo.gui.desktop`)
puts it in the host's menu with the icon `packaging/icons/org.halo.gui.svg`.

## Gotchas
- **Qt sends messages to journald, not the terminal**, when stderr is not a TTY (Arch's Qt is
  built with journald). A QML error leaves the process alive with no window and nothing in the
  log. To see them: `QT_FORCE_STDERR_LOGGING=1`, or `journalctl --user -o cat | grep qml`.
- **Reserved names in QML delegates:** `top` (an `Item` anchor) and `index` inside a nested
  `Repeater` give "Cannot override FINAL property". Here `isTop` and `modelData` are used.
- `Layout.*` outside a `Layout` (e.g. in the `footer`) is "Non-existent attached object" and takes
  the whole window down.
- Without an installed `.desktop` matching `desktopFileName`, the portal warns "App info not found
  for 'org.halo.gui'"; harmless.
- One `MouseArea` per ring half does not work: the lower one swallows the upper one's clicks. It is
  one per ring, and the half is decided by `mouse.y`.
