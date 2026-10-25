#!/bin/bash
# Gera dist/foco-<versão>-<arch>.tar.gz com os binários prontos: quem baixa roda ./install.sh sem
# precisar do Rust. Compile numa distro com glibc não mais nova que a das distros que você quer
# atender (Ubuntu 24.04 serve para todas as atuais).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
name="foco-$version-$(uname -m)"
out="dist/$name"
rm -rf "$out" "dist/$name.tar.gz"
mkdir -p "$out/bin" "$out/settings"

cargo build --release --locked --quiet
cp target/release/foco "$out/bin/"
# o miniaplicativo do COSMIC vai junto se já estiver compilado (./install.sh no COSMIC compila)
if [[ -x applet/target/release/foco-applet ]]; then
    cp applet/target/release/foco-applet "$out/bin/"
fi
cp settings/foco-settings.py "$out/settings/"
cp -r system data install.sh uninstall.sh README.md "$out/"
[[ -e LICENSE ]] && cp LICENSE "$out/"
find "$out" -name __pycache__ -prune -exec rm -rf {} +

tar -C dist -czf "dist/$name.tar.gz" "$name"
echo "dist/$name.tar.gz"
