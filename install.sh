#!/bin/bash
# Instala o Foco para o seu usuário (~/.local) nas distros atuais: Ubuntu, Debian, Mint, Pop!_OS,
# Fedora, Arch, Manjaro, openSUSE… e nos desktops GNOME, KDE, COSMIC, XFCE, Cinnamon, MATE,
# Hyprland e Sway. O que faltar de pacote é instalado com sudo (pergunta antes; -y aceita tudo).
# O bloqueio de sites é instalado depois, pela central (pede a senha uma vez).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

bin="$HOME/.local/bin"
share="${XDG_DATA_HOME:-$HOME/.local/share}"
autostart="${XDG_CONFIG_HOME:-$HOME/.config}/autostart"
desktop_env="${XDG_CURRENT_DESKTOP:-}"
desktop_env="${desktop_env^^}"
rust_min=1.88
yes=0
[[ ${1:-} == -y || ${1:-} == --yes ]] && yes=1

say() { printf '› %s\n' "$*"; }
note() { printf '  %s\n' "$*"; }

# sim/não; com -y aceita, sem terminal recusa (sudo e rustup precisariam de um)
ask() {
    [[ $yes == 1 ]] && return 0
    [[ -t 0 ]] || return 1
    local r
    read -r -p "  $1 [S/n] " r
    [[ -z $r || $r == [sSyY]* ]]
}

version_ge() { [[ "$(printf '%s\n%s\n' "$2" "$1" | sort -V | head -1)" == "$2" ]]; }

# ---------- distro ----------

pm=""
if [[ -r /etc/os-release ]]; then
    # shellcheck disable=SC1091
    ids=" $(. /etc/os-release; echo "${ID:-} ${ID_LIKE:-}") "
    case $ids in
        *" arch "* | *" manjaro "* | *" endeavouros "*) pm=pacman ;;
        *" fedora "* | *" rhel "* | *" centos "*) pm=dnf ;;
        *opensuse* | *" suse "* | *" sles "*) pm=zypper ;;
        *" debian "* | *" ubuntu "*) pm=apt ;;
    esac
fi

# nomes dos pacotes em cada família de distro
packages_for() {
    case $pm:$1 in
        apt:gtk) echo python3-gi gir1.2-gtk-4.0 gir1.2-adw-1 ;;
        dnf:gtk) echo python3-gobject gtk4 libadwaita ;;
        pacman:gtk) echo python-gobject gtk4 libadwaita ;;
        # no openSUSE o PyGObject leva a versão do Python no nome (python313-gobject)
        zypper:gtk) py=$(python3 -c 'import sys; print("python%d%d" % sys.version_info[:2])' 2>/dev/null || echo python3)
            echo "$py-gobject" "$py-gobject-Gdk" typelib-1_0-Gtk-4_0 typelib-1_0-Adw-1 \
                typelib-1_0-Graphene-1_0 typelib-1_0-GdkPixbuf-2_0 ;;
        apt:notify) echo libnotify-bin ;;
        dnf:notify | pacman:notify) echo libnotify ;;
        zypper:notify) echo libnotify-tools ;;
        pacman:sound) echo libpulse ;;
        *:sound) echo pulseaudio-utils ;;
        *:sound-theme) echo sound-theme-freedesktop ;;
        apt:pkexec | zypper:pkexec) echo pkexec ;;
        *:pkexec) echo polkit ;;
        dnf:ss) echo iproute ;;
        *:ss) echo iproute2 ;;
        apt:cc) echo gcc libc6-dev ;;
        *:cc) echo gcc ;;
        *:curl) echo curl ;;
        dnf:pkgconfig) echo pkgconf-pkg-config ;;
        pacman:pkgconfig | zypper:pkgconfig) echo pkgconf ;;
        *:pkgconfig) echo pkg-config ;;
    esac
}

install_packages() {
    case $pm in
        apt) sudo apt-get update -qq && sudo apt-get install -y "$@" ;;
        dnf) sudo dnf install -y "$@" ;;
        pacman) sudo pacman -S --needed --noconfirm "$@" ;;
        zypper) sudo zypper --non-interactive install "$@" ;;
    esac
}

gtk_ok() {
    python3 - 2>/dev/null <<'PY'
import gi
for ns, v in (("Gtk", "4.0"), ("Gsk", "4.0"), ("Graphene", "1.0"), ("Adw", "1"), ("GdkPixbuf", "2.0")):
    gi.require_version(ns, v)
from gi.repository import Adw, Gtk, Graphene, GdkPixbuf, Gsk  # noqa: F401
assert (Gtk.get_major_version(), Gtk.get_minor_version()) >= (4, 14)
assert (Adw.get_major_version(), Adw.get_minor_version()) >= (1, 4)
PY
}

[[ -x $HOME/.cargo/bin/cargo ]] && PATH="$HOME/.cargo/bin:$PATH"
prebuilt=0
[[ -x bin/foco ]] && prebuilt=1
rust_ok() {
    command -v cargo >/dev/null && command -v rustc >/dev/null &&
        version_ge "$(rustc --version | awk '{print $2}')" "$rust_min"
}

# ---------- o que falta ----------

# o miniaplicativo só faz sentido no painel do COSMIC
applet=0
if [[ ${FOCO_APPLET:-} == 1 ]] || { [[ ${FOCO_APPLET:-} != 0 ]] && { [[ $desktop_env == *COSMIC* ]] || command -v cosmic-panel >/dev/null; }; }; then
    applet=1
fi


say "conferindo o sistema"
needs=()
gtk_ok || needs+=(gtk)
command -v notify-send >/dev/null || needs+=(notify)
command -v pw-play >/dev/null || command -v paplay >/dev/null || needs+=(sound)
[[ -e /usr/share/sounds/freedesktop/stereo/complete.oga ]] || needs+=(sound-theme)
command -v pkexec >/dev/null || needs+=(pkexec)
command -v ss >/dev/null || [[ -x /usr/sbin/ss ]] || needs+=(ss)
if [[ $prebuilt == 0 ]]; then
    command -v cc >/dev/null || needs+=(cc)
    rust_ok || command -v curl >/dev/null || needs+=(curl)
    [[ $applet == 1 ]] && ! command -v pkg-config >/dev/null && needs+=(pkgconfig)
fi

if ((${#needs[@]})); then
    if [[ -z $pm ]]; then
        note "Não reconheci a distro. Instale pelo gerenciador de pacotes: GTK 4.14+ e libadwaita 1.4+"
        note "para Python (PyGObject), notify-send, paplay ou pw-play, o tema de sons freedesktop, pkexec,"
        note "ss (iproute2) e gcc. Depois rode este script de novo."
        exit 1
    fi
    pkgs=()
    for n in "${needs[@]}"; do
        read -r -a p <<<"$(packages_for "$n")"
        pkgs+=("${p[@]}")
    done
    say "faltam pacotes: ${pkgs[*]}"
    if ask "Instalar agora com sudo?"; then
        install_packages "${pkgs[@]}"
    else
        note "Instale e rode de novo. Sem eles algumas partes não funcionam."
        [[ " ${needs[*]} " == *" gtk "* || " ${needs[*]} " == *" cc "* ]] && exit 1
    fi
fi
if ! gtk_ok; then
    note "Esta distro tem GTK/libadwaita velhos demais para a central (precisa de GTK 4.14 e libadwaita 1.4)."
    note "O timer e o overlay funcionam; a central não abre. Use uma versão atual da distro."
fi

if [[ $prebuilt == 0 ]] && ! rust_ok; then
    if command -v rustup >/dev/null; then
        say "o Rust daqui é mais velho que o $rust_min"
        ask "Atualizar com rustup update stable?" && rustup update stable
    else
        say "para compilar o Foco é preciso o Rust $rust_min ou mais novo (o da distro não serve)"
        if ask "Instalar o Rust pelo rustup.rs, só para o seu usuário?"; then
            curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
            PATH="$HOME/.cargo/bin:$PATH"
        fi
    fi
    rust_ok || { note "Sem o Rust não dá para compilar. Instale em https://rustup.rs e rode de novo."; exit 1; }
fi

# ---------- compilar ----------

if [[ $prebuilt == 1 ]]; then
    say "usando os binários prontos de bin/"
    foco_bin=bin/foco
    applet_bin=bin/foco-applet
    [[ -x $applet_bin ]] || applet=0
else
    say "compilando"
    cargo build --release --quiet
    foco_bin=target/release/foco
    applet_bin=applet/target/release/foco-applet
    if [[ $applet == 1 ]]; then
        # sem o libxkbcommon-dev instalado, um .pc local apontando para a biblioteca do sistema basta
        applet_pc=""
        if ! pkg-config --exists xkbcommon 2>/dev/null; then
            lib=$(cc -print-file-name=libxkbcommon.so.0)
            if [[ -e $lib && $lib == /* ]]; then
                applet_pc="$PWD/applet/target/xkbcommon-shim"
                mkdir -p "$applet_pc/lib"
                ln -sf "$lib" "$applet_pc/lib/libxkbcommon.so"
                printf '%s\n' 'libdir=${pcfiledir}/lib' 'Name: xkbcommon' 'Description: libxkbcommon' \
                    'Version: 1.0.0' 'Libs: -L${libdir} -lxkbcommon' 'Cflags:' >"$applet_pc/xkbcommon.pc"
            fi
        fi
        say "compilando o miniaplicativo do painel (na primeira vez demora alguns minutos)"
        (cd applet && PKG_CONFIG_PATH="$applet_pc${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}" cargo build --release --quiet) ||
            { applet=0; note "o miniaplicativo não compilou; o resto do Foco funciona sem ele"; }
    fi
fi

# ---------- instalar ----------

say "instalando em $bin"
"$bin/foco" quit >/dev/null 2>&1 || true
install -Dm755 "$foco_bin" "$bin/foco"
install -Dm755 settings/foco-settings.py "$bin/foco-settings"
install -Dm644 data/foco.svg "$share/icons/hicolor/scalable/apps/foco.svg"
install -Dm755 system/foco-hosts "$share/foco/system/foco-hosts"
install -Dm644 system/dev.foco.hosts.policy "$share/foco/system/dev.foco.hosts.policy"
install -Dm755 system/install-bloqueio.sh "$share/foco/system/install-bloqueio.sh"
mkdir -p "$share/applications"
if [[ $applet == 1 ]]; then
    install -Dm755 "$applet_bin" "$bin/foco-applet"
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
fi

# no menu de aplicativos o Foco abre a central; no início da sessão só liga o timer
desktop() {
    cat <<EOF
[Desktop Entry]
Type=Application
Name=Foco
GenericName=Pomodoro
Comment=Pomodoro com overlay circular e bloqueio de sites
Exec=$1
Icon=foco
Categories=Utility;
Keywords=pomodoro;timer;foco;estudo;focus;
StartupNotify=false
Actions=toggle;
X-GNOME-Autostart-enabled=true

[Desktop Action toggle]
Name=Iniciar/pausar
Exec=$bin/foco toggle
EOF
}
desktop "$bin/foco open" >"$share/applications/dev.foco.Foco.desktop"
mkdir -p "$autostart"
if [[ ! -e $share/foco/.autostart-off ]]; then
    desktop "$bin/foco" >"$autostart/dev.foco.Foco.desktop"
fi
gtk-update-icon-cache -q -t "$share/icons/hicolor" 2>/dev/null || true
update-desktop-database -q "$share/applications" 2>/dev/null || true

say "iniciando"
setsid "$bin/foco" >/dev/null 2>&1 </dev/null &
sleep 0.8
"$bin/foco" status || true

# ---------- dicas do seu desktop ----------

echo
case :$PATH: in
    *":$bin:"*) ;;
    *) note "Para usar o comando foco no terminal, ponha $bin no PATH." ;;
esac
if [[ $applet == 1 ]] && ! grep -qs '"dev.foco.Applet"' "${XDG_CONFIG_HOME:-$HOME/.config}"/cosmic/com.system76.CosmicPanel.*/v1/plugins_*; then
    note "Para ver o Foco no painel: Configurações do COSMIC → Área de trabalho → Painel → Miniaplicativos → Adicionar → Foco."
fi
if [[ $desktop_env == *GNOME* ]] && ! gnome-extensions list --enabled 2>/dev/null | grep -qi appindicator; then
    note "No GNOME o ícone da bandeja precisa da extensão AppIndicator (o Ubuntu já traz). Sem ela, abra"
    note "o Foco pelo menu de aplicativos ou clicando no círculo."
fi
if [[ $desktop_env == *HYPRLAND* ]]; then
    note "Hyprland: para ligar com a sessão, ponha  exec-once = $bin/foco  no hyprland.conf."
elif [[ $desktop_env == *SWAY* ]]; then
    note "Sway: para ligar com a sessão, ponha  exec $bin/foco  no config."
fi
if [[ ! -x /usr/local/libexec/foco-hosts ]]; then
    note "Para bloquear sites: abra o Foco → Sites bloqueados → Instalar (pede a senha uma vez)."
    note "Sem um agente de senha do polkit (comum em Hyprland/Sway), rode no terminal:"
    note "  pkexec $share/foco/system/install-bloqueio.sh install"
fi
