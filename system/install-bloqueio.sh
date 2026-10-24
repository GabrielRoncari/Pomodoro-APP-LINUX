#!/bin/bash
# Instala (ou remove) o helper root do bloqueio de sites. Precisa de root:
#   pkexec ~/.local/share/foco/system/install-bloqueio.sh [install|uninstall]
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
helper=/usr/local/libexec/foco-hosts
policy=/usr/share/polkit-1/actions/dev.foco.hosts.policy

if [[ $EUID -ne 0 ]]; then
    echo "rode como root: pkexec $0 ${1:-install}" >&2
    exit 1
fi

case "${1:-install}" in
    install)
        install -D -o root -g root -m 0755 "$here/foco-hosts" "$helper"
        install -D -o root -g root -m 0644 "$here/dev.foco.hosts.policy" "$policy"
        echo "bloqueio instalado: $helper"
        ;;
    uninstall)
        if [[ -x $helper ]]; then
            "$helper" clear || true
        fi
        rm -f "$helper" "$policy"
        echo "bloqueio removido"
        ;;
    *)
        echo "uso: $0 [install|uninstall]" >&2
        exit 2
        ;;
esac
