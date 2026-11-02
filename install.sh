#!/usr/bin/env bash
# Installs Halo for the current user, without root: binary, user service and launchers.
# Hardware brightness also needs the udev rule (a root step, see the end).
#   ./install.sh [--no-build]
set -euo pipefail
cd "$(dirname "$0")"

bin="$HOME/.local/bin"
units="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
apps="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
data="${XDG_DATA_HOME:-$HOME/.local/share}/halo"

# Inside a distrobox there is no user systemd: ask the host.
host() { if [[ -n "${CONTAINER_ID:-}" ]]; then distrobox-host-exec "$@"; else "$@"; fi; }

[[ "${1:-}" == "--no-build" ]] || cargo build --release
install -Dm755 target/release/halo "$bin/halo"

# Chroma and Logitech DLLs for Proton games (halo proton install APPID). Need mingw-w64-gcc.
if [[ "${1:-}" != "--no-build" ]] && command -v x86_64-w64-mingw32-gcc >/dev/null; then
    make -s -C shims/chroma
    make -s -C shims/logitech
fi
if [[ -e shims/chroma/build/x64/RzChromaSDK64.dll && -e shims/logitech/build/x64/LogitechLed.dll ]]; then
    for arch in x64 x86; do
        install -Dm644 -t "$data/chroma/$arch" shims/chroma/build/$arch/Rz*.dll
        install -Dm644 -t "$data/logitech/$arch" shims/logitech/build/$arch/LogitechLed.dll
    done
else
    echo "warning: without mingw-w64-gcc the Chroma and Logitech DLLs were not built (games will not be able to drive the lights)"
fi

# The window (Qt 6 + Kirigami) only when it can be built. Inside a distrobox the binary uses the
# container's Qt, so the host gets a launcher that enters the container.
gui="$data/bin"
if [[ "${1:-}" != "--no-build" ]] && command -v qmake6 >/dev/null; then
    QMAKE="$(command -v qmake6)" cargo build --release -p halo-gui
fi
if [[ -e target/release/halo-gui ]]; then
    install -Dm755 target/release/halo-gui "$gui/halo-gui"
    if [[ -n "${CONTAINER_ID:-}" ]]; then
        cat > "$bin/halo-gui" <<WRAP
#!/usr/bin/env bash
# Launcher for Halo's window: the binary lives in the distrobox $CONTAINER_ID (Qt 6 + Kirigami).
if [[ -n "\${CONTAINER_ID:-}" ]]; then exec "$gui/halo-gui" "\$@"; fi
exec distrobox-enter -n "$CONTAINER_ID" -- "$gui/halo-gui" "\$@"
WRAP
        chmod 755 "$bin/halo-gui"
    else
        ln -sf "$gui/halo-gui" "$bin/halo-gui"
    fi
else
    rm -f "$apps/org.halo.gui.desktop"
    echo "warning: without qmake6 (Qt 6) the window was not built; the CLI and the daemon work the same"
fi

install -Dm644 packaging/systemd/halo.service "$units/halo.service"
install -Dm644 packaging/icons/org.halo.gui.svg "${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/scalable/apps/org.halo.gui.svg"
for f in packaging/desktop/*.desktop; do
    [[ "$f" == */org.halo.gui.desktop && ! -e target/release/halo-gui ]] && continue
    sed "s|@BIN@|$bin|g" "$f" > "$apps/$(basename "$f")"
done

host systemctl --user daemon-reload
host systemctl --user enable halo.service
host systemctl --user restart halo.service
sleep 1
"$bin/halo" status

rule=/etc/udev/rules.d/99-halo.rules
etc=/etc
[[ -n "${CONTAINER_ID:-}" ]] && etc=/run/host/etc  # the rule lives on the host
if [[ ! -e "${etc}${rule#/etc}" ]]; then
    cat <<MSG

One root step is missing for hardware brightness (run it in a terminal on the host):
  sed 's/@USER@/$USER/g' $PWD/packaging/udev/99-halo.rules.in | sudo tee $rule >/dev/null
  sudo udevadm control --reload && sudo udevadm trigger --subsystem-match=hidraw --action=change
MSG
fi
