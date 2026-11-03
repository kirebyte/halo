# The Decky plugin

[Decky Loader](https://github.com/SteamDeckHomebrew/decky-loader) adds plugins to the quick access
menu of Steam's game mode (the "…" button). Halo's plugin (`decky/`) puts the essentials there,
without leaving the game: the current colors, "Rings off", hardware brightness, the effect and its
speed, a row of base colors, and whether games may paint the rings. It follows Steam's language
(English, Spanish, French, Japanese) with the same strings as the CLI and the window.

It is a thin layer over the daemon. `main.py` (the backend Decky runs as the user; the plugin has no
`root` flag) calls `org.halo.Halo1` on the session bus with `busctl --json=short`, unwrapping its
`{type, data}` output, and `src/index.tsx` (React, `@decky/ui`) polls `state` every 1.5 s while the
panel is open and after each change. Setters return the daemon's error message, shown as a toast.
Without the daemon the panel says so.

## Build
Needs node and npm (in the distrobox: `pacman -Syu --needed nodejs npm`):
```bash
decky/build.sh            # npm ci + rollup, then out/halo/ and out/halo.zip
```
`out/halo/` is the plugin folder (`plugin.json`, `main.py`, `dist/index.js`, `locales/`, the
license); `out/halo.zip` is the same, for Decky's installer.

## Install
Decky Loader first: on Bazzite, `ujust setup-decky install` (it asks for the password), then
restart into game mode once. Then one of:
- Decky's settings → Developer → "Install plugin from ZIP" (needs developer mode) with
  `decky/out/halo.zip`.
- By hand, on the host (the plugins folder belongs to root):
  ```bash
  sudo cp -r decky/out/halo ~/homebrew/plugins/halo
  sudo systemctl restart plugin_loader
  ```
The plugin needs the daemon (`systemctl --user status halo`), which `install.sh` enables.

## Development
`npm run watch` in `decky/` rebuilds `dist/index.js` on every change; copy it over
`~/homebrew/plugins/halo/dist/` and reload the plugin from Decky's developer menu. The backend can
be driven outside Decky with a stub `decky` module exposing `logger`, `DECKY_USER` and
`DECKY_PLUGIN_DIR`, against the real daemon. Decky's log is `~/homebrew/logs/` and the plugin's
own messages go to `decky.logger`.
