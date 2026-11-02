# Contributing

## Build and test
```bash
cargo build --release          # halo-core + halo (CLI and daemon); no Qt needed
cargo test
QMAKE=/usr/bin/qmake6 cargo build --release -p halo-gui   # the window; needs Qt 6 + Kirigami
make -C shims/chroma && make -C shims/logitech            # the Wine/Proton DLLs; needs mingw-w64
./install.sh --no-build        # install what is already built and restart the daemon
```
`halo --backend dummy ...` exercises the CLI without hardware. The daemon logs to the journal:
`journalctl --user -u halo -f`.

## Code
- Rust 2024, no warnings. Comments, logs and identifiers in English.
- The daemon never polls when nothing is animated; new features should wake on events (inotify,
  pidfd, D-Bus signals, sockets) or sleep until the next frame.
- Bridges paint exactly what the game asks for. Only Halo's own effects keep under 3 flashes per
  second (WCAG 2.3.1).
- Each LED lights half a ring, so dimming a zone makes the ring look cut off. Effects should vary
  hue rather than brightness.

## Translations
User-facing text goes through `tr!("English text")` (Rust) or `Halo.tr("English text")` (QML);
placeholders are `{name}`. Daemon logs stay in English. Tables are flat JSON in `locales/`:
`en.json` only holds ids that are not sentences (`effect.rainbow`, `speed.fast`, `usage`); the
other languages hold every key. `cargo test -p halo-core` checks that the tables agree with each
other and keep their placeholders. To add a language, add `locales/xx.json` and list it in
`crates/halo-core/src/i18n.rs`.

## Pull requests
Keep them focused. Say what you tested on hardware and what you could not.
