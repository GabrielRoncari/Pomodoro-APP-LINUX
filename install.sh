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

# o miniaplicativo do painel usa o libcosmic; sem o libxkbcommon-dev instalado, um .pc
# local apontando para a biblioteca do sistema basta para linkar
applet_pc=""
if ! pkg-config --exists xkbcommon 2>/dev/null; then
    lib=$(cc -print-file-name=libxkbcommon.so.0)
    if [[ -e "$lib" ]]; then
        applet_pc="$PWD/applet/target/xkbcommon-shim"
        mkdir -p "$applet_pc/lib"
        ln -sf "$lib" "$applet_pc/lib/libxkbcommon.so"
        printf '%s\n' 'libdir=${pcfiledir}/lib' 'Name: xkbcommon' 'Description: libxkbcommon' \
            'Version: 1.0.0' 'Libs: -L${libdir} -lxkbcommon' 'Cflags:' >"$applet_pc/xkbcommon.pc"
    fi
fi
echo "› compilando o miniaplicativo do painel (na primeira vez demora alguns minutos)"
applet_ok=1
(cd applet && PKG_CONFIG_PATH="$applet_pc${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}" cargo build --release --quiet) ||
    applet_ok=0

echo "› instalando em $bin"
"$bin/foco" quit >/dev/null 2>&1 || true
install -Dm755 target/release/foco "$bin/foco"
install -Dm755 settings/foco-settings.py "$bin/foco-settings"
install -Dm644 data/foco.svg "$share/icons/hicolor/scalable/apps/foco.svg"
install -Dm755 system/foco-hosts "$share/foco/system/foco-hosts"
install -Dm644 system/dev.foco.hosts.policy "$share/foco/system/dev.foco.hosts.policy"
install -Dm755 system/install-bloqueio.sh "$share/foco/system/install-bloqueio.sh"
if [[ $applet_ok == 1 ]]; then
    install -Dm755 applet/target/release/foco-applet "$bin/foco-applet"
    cat >"$share/applications/dev.foco.Applet.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Foco
Comment=Pomodoro: tempo no painel, controles e bloqueio de sites
Exec=$bin/foco-applet
Icon=foco
NoDisplay=true
Categories=COSMIC;Utility;
Keywords=COSMIC;Applet;Pomodoro;Foco;Timer;
X-CosmicApplet=true
X-CosmicHoverPopup=Auto
X-HostWaylandDisplay=true
EOF
    # o painel reabre o applet já com o binário novo
    pkill -x foco-applet 2>/dev/null || true
else
    echo "  (o miniaplicativo não compilou; o resto do Foco funciona sem ele)"
fi

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

panels="${XDG_CONFIG_HOME:-$HOME/.config}/cosmic"
if [[ $applet_ok == 1 ]] && ! grep -qs '"dev.foco.Applet"' "$panels"/com.system76.CosmicPanel.*/v1/plugins_*; then
    echo
    echo "Para ver o Foco no painel: Configurações do COSMIC → Área de trabalho → Painel → Miniaplicativos → Adicionar → Foco."
fi
if [[ ! -x /usr/local/libexec/foco-hosts ]]; then
    echo
    echo "Para bloquear sites: abra as configurações (foco settings) → Bloqueio → Instalar."
fi
