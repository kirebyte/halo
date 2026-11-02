# Halo

Lighting for the joystick rings of the **ROG Xbox Ally** on Linux, without Armoury Crate: color per
zone, effects, per-game profiles, and **games driving the lights** during gameplay as they do on
Windows (Razer Chroma, SteelSeries GameSense, Logitech LED, OpenRGB). Written in Rust for
Bazzite/SteamOS; no root needed for the essentials.

## Features
- **Color per zone** (four half rings: left bottom/top, right top/bottom), hardware brightness and
  a "rings off" switch.
- **15 effects** ([docs/effects.md](docs/effects.md)): breathing, cycle, rainbow, comet, strobe,
  tide, starry sky, star, clock, sun, sensors (temperature and CPU), battery, and two that follow
  the music (equalizer and pulse). Breathing, cycle and rainbow are animated by the Ally's MCU at
  0 % CPU. Custom colors and direction per effect; everything turns off with the screen.
- **Games control the lights** ([docs/bridges.md](docs/bridges.md)): bridges for Razer Chroma (a
  drop-in DLL for the Proton prefix, plus REST), SteelSeries GameSense, the Logitech LED SDK and an
  OpenRGB server for Artemis and friends. Global and per-game permission.
- **Per-game profiles** ([docs/profiles.md](docs/profiles.md)): when a Steam game opens, its color
  or effect; when it closes, the usual lights. No polling: inotify + pidfd.
- **CLI** (`halo`), a user **daemon** with a D-Bus interface (`org.halo.Halo1`) and a Qt/Kirigami
  **window** ([docs/gui.md](docs/gui.md)), in English, Spanish, French and Japanese. The color
  comes back after suspend.

With nothing animated the daemon uses no CPU: it only wakes on events (and reads the screen state
once per second, if that option is on).

## Hardware
Only the ROG Xbox Ally (`0B05:1B4C`) so far. The rings are driven through **HID LampArray** (USB
interface 00, [docs/lamparray.md](docs/lamparray.md)), which is user-accessible out of the box, so
color needs neither root nor a udev rule. The udev rule in `packaging/udev/` only opens the vendor
channel for hardware brightness and the MCU effects. The backend is a trait, in case other devices
are added one day.

## Install
Needs Rust (edition 2024). On Bazzite, inside a distrobox (e.g. Arch):
```bash
pacman -Syu --needed rust mingw-w64-gcc                       # mingw: the Chroma/Logitech DLLs
pacman -Syu --needed qt6-base qt6-declarative qt6-svg kirigami kirigami-addons qqc2-breeze-style breeze-icons cmake  # the window (optional)
git clone https://github.com/kirebyte/halo && cd halo
./install.sh
```
This leaves `~/.local/bin/halo`, the user service `halo.service`, the window in the menu and the
launchers. At the end it prints the one root step (the udev rule) to run by hand.
`./uninstall.sh` removes everything.

## Quick use
```bash
halo set red                        # one color (a name or RRGGBB)
halo set red green blue yellow      # one per zone
halo effect tide fast               # an effect; `halo effect` lists them
halo effect off
halo brightness 2                   # 0 turns the rings off
halo games                          # connected and blocked games
halo proton list                    # Proton games and whether they have the bridge
halo proton install 427520          # put the Chroma/GameSense/Logitech bridge in that game
halo profile 427520 tide            # when Factorio opens, tide
halo config                         # halo.conf: latitude, screen_off, hardware_effects...
halo status
```
Configuration lives in `~/.config/halo/halo.conf` (`key = value`). The language follows
`LANG`/`LC_MESSAGES`.

## How it is built
```
crates/halo-core   colors, frames, HID discovery, backends (lamparray, vendor, dummy), translations
crates/halo        the CLI and the daemon: layer compositor, D-Bus, effects, bridges, Steam, Proton
crates/halo-gui    the window (cxx-qt + Kirigami); only built on purpose
shims/             the Chroma and Logitech DLLs for Wine/Proton (mingw)
locales/           en, es, fr, ja
packaging/         systemd, udev, .desktop, icon
```
The daemon mixes **layers** by priority: base color < effect (100) < game profile (120) < external
tools (150) < games (200). Each zone takes the highest layer that covers it. See
[CONTRIBUTING.md](CONTRIBUTING.md) for building, testing and adding translations.

## License and credits
GPL-2.0-or-later. The vendor protocol bytes come from Luke Jones's `hid-asus-ally` driver
(GPL-2.0-or-later). Chroma, GameSense and Logitech are implemented from their public documentation;
no third-party binary is redistributed. "Aura" and "ROG" are trademarks of ASUS; Halo is not
affiliated with ASUS, Razer, SteelSeries or Logitech.
