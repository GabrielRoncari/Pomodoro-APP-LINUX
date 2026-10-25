# Foco

Pomodoro para o COSMIC (Pop!_OS 24.04, Wayland): um timer circular flutuante que fica por
cima de tudo, uma central com os controles e as configurações, um miniaplicativo no painel e
bloqueio de sites durante o foco.

- **Overlay circular** em layer-shell: sempre visível (até sobre tela cheia, se quiser), não
  rouba o foco do teclado, aparece em todas as áreas de trabalho.
  - arraste para mover · roda do mouse ou a alça ↘ para redimensionar
  - clique (qualquer botão) abre a central; clicar de novo fecha
  - opção de travar (os cliques atravessam) e de escolher o monitor
- **Central**: o timer grande com iniciar/pausar, recomeçar, pular e as etapas, atalhos para o
  bloqueio e o modo rígido, um botão para mostrar/esconder o overlay, o resumo do dia e todas as configurações. Ao iniciar, ela
  sai da frente; Esc também fecha. O fundo é uma imagem sua (Overlay e aparência → Fundo desta
  janela) ou um vermelho.
- **Miniaplicativo do painel**: anel de progresso e o tempo que falta no painel do COSMIC; o
  clique abre os controles rápidos (iniciar/pausar, pular, etapas, overlay) e a central.
- **Bandeja** (opcional, desligável em Overlay e aparência → Painel): o mesmo anel, com um menu.
- **Bloqueio de sites** durante o foco, em qualquer navegador. Nas pausas tudo volta.
  Abas que já estavam abertas são cortadas na hora.
- **Modo rígido**: durante o foco não dá para pausar, pular nem liberar sites.
- Ciclo com pausa longa, início automático, notificação e som, histórico do dia/semana.

## Instalar

```sh
./install.sh
```

Compila (precisa de `cargo`), instala em `~/.local/bin`, cria o atalho no menu de aplicativos
e o início automático, e já abre o Foco. Não precisa de root.

O miniaplicativo (`applet/`, feito com o libcosmic) leva alguns minutos para compilar na primeira
vez. Para pô-lo no painel: Configurações do COSMIC → Área de trabalho → Painel → Miniaplicativos →
Adicionar → **Foco**. Ele só mostra e controla: o timer continua no daemon `foco`.

O **bloqueio de sites** precisa de um helper root, instalado uma vez: abra a central
(clique no overlay, ou `foco settings`) → **Sites bloqueados** → **Instalar**. A senha é pedida
só nessa hora; depois o Foco bloqueia e libera sozinho.

## Linha de comando

```sh
foco                 # inicia (se já estiver rodando, mostra o overlay)
foco toggle          # inicia/pausa: bom para um atalho de teclado
foco skip | reset | reset-cycle
foco phase focus|short|long
foco show | hide | lock on|off | size 220 | output DP-1
foco status [--json]
foco settings        # abre a central
foco quit
```

Atalho de teclado: Configurações do COSMIC → Teclado → Atalhos personalizados → comando
`foco toggle`.

## Como o bloqueio funciona

`/usr/local/libexec/foco-hosts` escreve um bloco marcado no `/etc/hosts` apontando os sites
(e os subdomínios `www.` e `m.`) para `0.0.0.0`, e derruba as conexões abertas com eles
(`ss -K`). A política do polkit `dev.foco.hosts` libera só esse helper, só para a sessão ativa
e sem senha. O helper só aceita nomes de domínio válidos e só mexe no próprio bloco.

Ele serve para disciplina, não como barreira de segurança: VPN, proxy ou DNS-over-HTTPS
configurado à mão num navegador podem passar por cima.

## Arquivos

| | |
|---|---|
| `~/.config/foco/config.json` | preferências (quem escreve é a central) |
| `~/.local/share/foco/fundo-*.jpg` | cópia reduzida da imagem de fundo da central |
| `~/.local/state/foco/state.json` | timer e posição do overlay (quem escreve é o daemon) |
| `~/.local/share/foco/history.jsonl` | focos concluídos |
| `~/.local/state/foco/foco.log` | início, fim e erros do daemon |
| `$XDG_RUNTIME_DIR/foco.sock` | socket de controle |

## Desinstalar

```sh
./uninstall.sh
```

## Código

- `src/` — daemon em Rust: `app.rs` (laço, overlay Wayland, mouse), `render.rs` (desenho com
  tiny-skia), `tray.rs` (StatusNotifierItem), `timer.rs`, `blocker.rs`, `ipc.rs`
- `applet/` — miniaplicativo do painel (libcosmic), conversa com o daemon pelo socket
- `settings/foco-settings.py` — a central, em GTK4 + libadwaita (o mostrador é desenhado com
  caminhos do Gsk, sem cairo)
- `system/` — helper root, política do polkit e instalador do bloqueio

Fontes Fira Sans embutidas (SIL Open Font License).
