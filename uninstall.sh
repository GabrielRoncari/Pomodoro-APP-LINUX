#!/bin/bash
# Remove o Foco do seu usuário. Se o bloqueio de sites estiver instalado, pede a senha para removê-lo.
set -euo pipefail

bin="$HOME/.local/bin"
share="${XDG_DATA_HOME:-$HOME/.local/share}"
autostart="${XDG_CONFIG_HOME:-$HOME/.config}/autostart"

"$bin/foco" quit >/dev/null 2>&1 || true
if [[ -x /usr/local/libexec/foco-hosts ]]; then
    pkexec "$share/foco/system/install-bloqueio.sh" uninstall || echo "bloqueio de sites mantido"
fi
rm -f "$bin/foco" "$bin/foco-settings" \
    "$share/applications/dev.foco.Foco.desktop" \
    "$autostart/dev.foco.Foco.desktop" \
    "$share/icons/hicolor/scalable/apps/foco.svg"
rm -rf "$share/foco/system"
echo "Foco removido. Configuração e histórico ficaram em ~/.config/foco e ~/.local/share/foco."
