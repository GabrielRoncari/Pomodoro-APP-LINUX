# Foco

Pomodoro para Linux com um timer circular que flutua por cima de tudo, uma central com os
controles e as configurações, e bloqueio de sites durante o foco.

- **Overlay circular**: sempre visível, não rouba o foco do teclado, aparece em todas as áreas
  de trabalho.
  - arraste para mover · roda do mouse ou a alça ↘ para redimensionar
  - clique (qualquer botão) abre a central; clicar de novo fecha
  - dá para travar (os cliques atravessam) e escolher o monitor
- **Central**: o timer grande com iniciar/pausar, recomeçar, pular e as etapas, atalhos para o
  bloqueio e o modo rígido, um botão para mostrar/esconder o overlay, o resumo do dia e todas as
  configurações. Ao iniciar um foco ela sai da frente; Esc também fecha. O fundo pode ser uma
  imagem sua ou um vermelho.
- **Bloqueio de sites** durante o foco, em qualquer navegador. Nas pausas tudo volta. Abas que já
  estavam abertas são cortadas na hora.
- **Modo rígido**: durante o foco não dá para pausar, pular nem liberar sites.
- **Bandeja** (opcional) com o anel do tempo, e **miniaplicativo do painel** no COSMIC.
- Ciclo com pausa longa, início automático, notificação e som, histórico do dia e da semana.

## Onde funciona

Distros atuais: Ubuntu 24.04+, Debian 13+, Linux Mint 22+, Pop!_OS 24.04, Fedora 40+, Arch,
Manjaro, EndeavourOS, openSUSE Tumbleweed e derivadas. Precisa de GTK 4.14 e libadwaita 1.4.

| Desktop | Overlay | Observação |
|---|---|---|
| COSMIC, KDE Plasma, Hyprland, Sway, niri, labwc, Wayfire | Wayland (layer-shell) | fica por cima até de tela cheia |
| GNOME (Wayland) | pelo Xwayland | bandeja precisa da extensão AppIndicator (o Ubuntu já traz) |
| Cinnamon, XFCE, MATE, KDE em X11, i3… | X11 | sem compositor o círculo fica sem transparência nas bordas |

No GNOME, sem bandeja, o Foco abre pelo menu de aplicativos ou clicando no círculo.

## Instalar

```sh
git clone https://github.com/GabrielRoncari/Pomodoro-APP-LINUX.git foco && cd foco
./install.sh
```

O instalador:

- confere o que falta e, com a sua permissão, instala pelo `apt`, `dnf`, `pacman` ou `zypper`
  (GTK 4/libadwaita para Python, notify-send, paplay, o tema de sons, pkexec);
- compila o Foco (`cargo`). Se o Rust da distro for velho (precisa do 1.88+), oferece instalar
  pelo [rustup](https://rustup.rs) só para o seu usuário;
- instala tudo em `~/.local` (sem root), cria o atalho no menu e o início automático, e liga o Foco.

`./install.sh -y` aceita tudo sem perguntar. Num pacote de release (com a pasta `bin/`) ele usa os
binários prontos e não precisa do Rust; `scripts/dist.sh` gera esse pacote.

O **bloqueio de sites** precisa de um helper root, instalado uma vez: abra o Foco → **Sites
bloqueados** → **Instalar**. A senha é pedida só nessa hora; depois o Foco bloqueia e libera
sozinho. Em Hyprland/Sway sem agente de senha do polkit, rode no terminal:

```sh
pkexec ~/.local/share/foco/system/install-bloqueio.sh install
```

**Hyprland/Sway** não leem `~/.config/autostart`: para ligar com a sessão, ponha
`exec-once = ~/.local/bin/foco` no `hyprland.conf` ou `exec ~/.local/bin/foco` no config do Sway.

**COSMIC**: o instalador compila também o miniaplicativo do painel. Para pô-lo no painel:
Configurações do COSMIC → Área de trabalho → Painel → Miniaplicativos → Adicionar → **Foco**.

## Linha de comando

```sh
foco                 # liga (se já estiver rodando, mostra o overlay)
foco open            # liga se preciso e abre a central (é o que o menu de aplicativos faz)
foco toggle          # inicia/pausa: bom para um atalho de teclado
foco skip | reset | reset-cycle
foco phase focus|short|long
foco show | hide | lock on|off | size 220 | output DP-1
foco status [--json]
foco settings        # abre a central
foco quit
```

Atalho de teclado: nas configurações de teclado do seu desktop, crie um atalho com o comando
`foco toggle`. A central mostra onde fica no seu.

## Como o bloqueio funciona

`/usr/local/libexec/foco-hosts` escreve um bloco marcado no `/etc/hosts` apontando os sites
(e os subdomínios `www.` e `m.`) para `0.0.0.0`, e derruba as conexões abertas com eles
(`ss -K`, quando o kernel permite). A política do polkit `dev.foco.hosts` libera só esse helper,
só para a sessão ativa e sem senha. O helper só aceita nomes de domínio válidos e só mexe no
próprio bloco; no Fedora ele restaura o rótulo do SELinux e, se houver `nscd`, limpa o cache.

Ele serve para disciplina, não como barreira de segurança: VPN, proxy ou DNS-over-HTTPS
configurado à mão num navegador podem passar por cima.

## Arquivos

| | |
|---|---|
| `~/.config/foco/config.json` | preferências (quem escreve é a central) |
| `~/.local/state/foco/state.json` | timer e posição do overlay (quem escreve é o daemon) |
| `~/.local/share/foco/history.jsonl` | focos concluídos |
| `~/.local/share/foco/fundo-*.jpg` | cópia reduzida da imagem de fundo da central |
| `~/.local/state/foco/foco.log` | início, fim e erros do daemon |
| `$XDG_RUNTIME_DIR/foco.sock` | socket de controle |

## Desinstalar

```sh
./uninstall.sh
```

## Código

- `src/` — daemon em Rust: `app.rs` (timer, mouse, laço), `wl.rs` (overlay no Wayland com
  layer-shell), `x11.rs` (overlay numa janela X11 sempre por cima: GNOME pelo Xwayland e sessões
  X11), `render.rs` (desenho com tiny-skia), `tray.rs` (StatusNotifierItem), `timer.rs`,
  `blocker.rs`, `ipc.rs`
- `applet/` — miniaplicativo do painel do COSMIC (libcosmic), conversa com o daemon pelo socket
- `settings/foco-settings.py` — a central, em GTK4 + libadwaita (o mostrador é desenhado com
  caminhos do Gsk, sem cairo)
- `system/` — helper root, política do polkit e instalador do bloqueio
- `scripts/dist.sh` — gera o pacote de release com binários prontos

## Licença

GPL-3.0 ou posterior (veja `LICENSE`). As fontes Fira Sans embutidas em `assets/` são da Mozilla,
sob a SIL Open Font License 1.1 (`assets/OFL.txt`).
