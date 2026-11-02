#!/usr/bin/env bash
# Removes what install.sh put in place. Keeps the saved state (~/.local/state/halo), the
# configuration (~/.config/halo) and the udev rule.
set -euo pipefail
host() { if [[ -n "${CONTAINER_ID:-}" ]]; then distrobox-host-exec "$@"; else "$@"; fi; }

data="${XDG_DATA_HOME:-$HOME/.local/share}"
"$HOME/.local/bin/halo" proton remove --all 2>/dev/null || true
"$HOME/.local/bin/halo" release 2>/dev/null || true
host systemctl --user disable --now halo.service 2>/dev/null || true
rm -f "${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/halo.service"
rm -f "$data"/applications/{halo-color,halo-eq,org.halo.gui}.desktop
rm -f "$data/icons/hicolor/scalable/apps/org.halo.gui.svg"
rm -f "$HOME/.local/bin/halo" "$HOME/.local/bin/halo-gui"
rm -rf "$data/halo/bin" "$data/halo/chroma" "$data/halo/logitech"
host systemctl --user daemon-reload
echo "Halo uninstalled. For the udev rule (root): sudo rm /etc/udev/rules.d/99-halo.rules"
