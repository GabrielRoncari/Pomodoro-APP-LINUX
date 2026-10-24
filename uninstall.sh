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
# tira o miniaplicativo dos painéis antes de apagar o binário
python3 - <<'PY'
import os, pathlib, re
cfg = pathlib.Path(os.environ.get("XDG_CONFIG_HOME") or pathlib.Path.home() / ".config")
for f in cfg.glob("cosmic/com.system76.CosmicPanel.*/v1/plugins_*"):
    text = f.read_text()
    new = re.sub(r'\n\s*"dev\.foco\.Applet",', "", text)
    if new != text:
        f.write_text(new)
PY
pkill -x foco-applet 2>/dev/null || true
rm -f "$bin/foco" "$bin/foco-settings" "$bin/foco-applet" \
    "$share/applications/dev.foco.Foco.desktop" "$share/applications/dev.foco.Applet.desktop" \
    "$autostart/dev.foco.Foco.desktop" \
    "$share/icons/hicolor/scalable/apps/foco.svg"
rm -rf "$share/foco/system"
echo "Foco removido. Configuração e histórico ficaram em ~/.config/foco e ~/.local/share/foco."
