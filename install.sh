#!/bin/bash
# Compila e instala o Foco para o seu usuário (~/.local). Não precisa de root.
# O bloqueio de sites é instalado à parte, pela tela de configurações (pede a senha uma vez).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

bin="$HOME/.local/bin"
share="${XDG_DATA_HOME:-$HOME/.local/share}"
autostart="${XDG_CONFIG_HOME:-$HOME/.config}/autostart"

echo "› compilando"
cargo build --release --quiet

echo "› instalando em $bin"
"$bin/foco" quit >/dev/null 2>&1 || true
install -Dm755 target/release/foco "$bin/foco"
install -Dm755 settings/foco-settings.py "$bin/foco-settings"
install -Dm644 data/foco.svg "$share/icons/hicolor/scalable/apps/foco.svg"
install -Dm755 system/foco-hosts "$share/foco/system/foco-hosts"
install -Dm644 system/dev.foco.hosts.policy "$share/foco/system/dev.foco.hosts.policy"
install -Dm755 system/install-bloqueio.sh "$share/foco/system/install-bloqueio.sh"

desktop() {
    cat <<EOF
[Desktop Entry]
Type=Application
Name=Foco
GenericName=Pomodoro
Comment=Pomodoro com overlay circular e bloqueio de sites
Exec=$bin/foco
Icon=foco
Categories=Utility;Education;
Keywords=pomodoro;timer;foco;estudo;
StartupNotify=false
Actions=settings;toggle;

[Desktop Action settings]
Name=Configurações
Exec=$bin/foco-settings

[Desktop Action toggle]
Name=Iniciar/pausar
Exec=$bin/foco toggle
EOF
}
desktop >"$share/applications/dev.foco.Foco.desktop"
mkdir -p "$autostart"
if [[ ! -e "$autostart/dev.foco.Foco.desktop" && ! -e "$share/foco/.autostart-off" ]]; then
    desktop >"$autostart/dev.foco.Foco.desktop"
fi
gtk-update-icon-cache -q -t "$share/icons/hicolor" 2>/dev/null || true
update-desktop-database -q "$share/applications" 2>/dev/null || true

echo "› iniciando"
setsid "$bin/foco" >/dev/null 2>&1 < /dev/null &
sleep 0.6
"$bin/foco" status || true

if [[ ! -x /usr/local/libexec/foco-hosts ]]; then
    echo
    echo "Para bloquear sites: abra as configurações (foco settings) → Bloqueio → Instalar."
fi
