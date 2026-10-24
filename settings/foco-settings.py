#!/usr/bin/python3
"""Configurações do Foco (GTK4 + libadwaita).

Escreve ~/.config/foco/config.json e avisa o daemon pelo socket. O que é do overlay
(visível, travado, tamanho, monitor) vai direto por comando, porque o daemon é o dono.
"""

import copy
import datetime
import json
import os
import shutil
import socket
import sys
from pathlib import Path

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
from gi.repository import Adw, Gdk, Gio, GLib, Gtk  # noqa: E402

HOME = Path.home()


def xdg(var, fallback):
    value = os.environ.get(var)
    return Path(value) if value else HOME / fallback


CONFIG = xdg("XDG_CONFIG_HOME", ".config") / "foco" / "config.json"
HISTORY = xdg("XDG_DATA_HOME", ".local/share") / "foco" / "history.jsonl"
AUTOSTART = xdg("XDG_CONFIG_HOME", ".config") / "autostart" / "dev.foco.Foco.desktop"
# o install.sh não recria o autostart se você desligou aqui
AUTOSTART_OFF = xdg("XDG_DATA_HOME", ".local/share") / "foco" / ".autostart-off"
RUNTIME = os.environ.get("XDG_RUNTIME_DIR")
SOCKET = Path(RUNTIME) / "foco.sock" if RUNTIME else Path(f"/tmp/foco-{os.getuid()}.sock")

DEFAULT_COLORS = {"focus_color": "#ff6b5e", "short_break_color": "#3fd8a0", "long_break_color": "#62a8ff"}

DEFAULTS = {
    "focus_min": 25,
    "short_break_min": 5,
    "long_break_min": 15,
    "long_break_every": 4,
    "auto_start_breaks": True,
    "auto_start_focus": False,
    "strict_mode": False,
    "appearance": {
        **DEFAULT_COLORS,
        "background_opacity": 0.86,
        "idle_opacity": 0.94,
        "above_fullscreen": True,
        "tray_icon": True,
    },
    "block": {
        "enabled": True,
        "sites": ["instagram.com", "facebook.com", "tiktok.com", "x.com", "twitter.com", "reddit.com", "threads.com", "threads.net"],
        "kill_connections": True,
    },
    "alerts": {"notifications": True, "sound": True, "volume": 0.8},
}

PRESETS = [
    ("Redes sociais", "user-available-symbolic",
     ["instagram.com", "facebook.com", "tiktok.com", "x.com", "twitter.com", "reddit.com", "threads.com", "threads.net"]),
    ("Vídeos e streaming", "video-display-symbolic",
     ["youtube.com", "twitch.tv", "kick.com", "netflix.com", "primevideo.com", "disneyplus.com", "max.com"]),
    ("Mensagens", "mail-unread-symbolic",
     ["discord.com", "web.whatsapp.com", "web.telegram.org"]),
    ("Compras", "emblem-default-symbolic",
     ["mercadolivre.com.br", "shopee.com.br", "amazon.com.br", "aliexpress.com", "shein.com"]),
    ("Notícias e fofoca", "emblem-documents-symbolic",
     ["g1.globo.com", "ge.globo.com", "uol.com.br", "metropoles.com", "terra.com.br"]),
]

CSS = """
.stat-value { font-size: 1.6em; font-weight: 700; font-feature-settings: "tnum"; }
.stat-label { opacity: 0.7; font-size: 0.85em; }
.stat-card { padding: 14px 10px; border-radius: 14px; background: alpha(currentColor, 0.06); }
.hint { opacity: 0.75; }
"""


def merged(base, override):
    out = copy.deepcopy(base)
    for key, value in (override or {}).items():
        if isinstance(value, dict) and isinstance(out.get(key), dict):
            out[key] = merged(out[key], value)
        else:
            out[key] = value
    return out


def load_config():
    try:
        return merged(DEFAULTS, json.loads(CONFIG.read_text()))
    except FileNotFoundError:
        return copy.deepcopy(DEFAULTS)
    except (OSError, ValueError) as e:
        print(f"foco-settings: config inválida ({e}), usando padrões", file=sys.stderr)
        return copy.deepcopy(DEFAULTS)


def save_config(cfg):
    CONFIG.parent.mkdir(parents=True, exist_ok=True)
    tmp = CONFIG.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(cfg, indent=2, ensure_ascii=False) + "\n")
    tmp.replace(CONFIG)


def normalize_site(raw):
    """Mesmo critério do daemon: só o domínio, sem esquema, caminho, porta ou www."""
    s = raw.strip().lower()
    if "://" in s:
        s = s.split("://", 1)[1]
    for sep in "/?#":
        s = s.split(sep, 1)[0]
    s = s.rsplit(":", 1)[0] if ":" in s else s
    s = s.removeprefix("*.").removeprefix("www.").rstrip(".")
    labels = s.split(".")
    ok = (
        len(labels) >= 2
        and len(s) <= 253
        and all(
            0 < len(label) <= 63
            and not label.startswith("-")
            and not label.endswith("-")
            and all(c.isascii() and (c.isalnum() or c == "-") for c in label)
            for label in labels
        )
    )
    return s if ok else None


def ipc(command, timeout=1.5):
    """Manda um comando ao daemon; devolve o JSON da resposta ou None se ele não está rodando."""
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
            s.settimeout(timeout)
            s.connect(str(SOCKET))
            s.sendall((command + "\n").encode())
            data = b""
            while not data.endswith(b"\n"):
                chunk = s.recv(65536)
                if not chunk:
                    break
                data += chunk
        return json.loads(data or b"null")
    except (OSError, ValueError):
        return None


def find_file(*names):
    here = Path(__file__).resolve().parent
    for base in (here.parent / "system", xdg("XDG_DATA_HOME", ".local/share") / "foco" / "system", here):
        for name in names:
            if (base / name).exists():
                return base / name
    return None


def foco_binary():
    here = Path(__file__).resolve().parent
    for p in (here / "foco", here.parent / "target" / "release" / "foco", HOME / ".local/bin/foco"):
        if p.exists():
            return str(p)
    return shutil.which("foco") or "foco"


def fmt_minutes(m):
    return f"{m // 60}h{m % 60:02d}" if m >= 60 else f"{m} min"


def read_stats():
    entries = []
    try:
        for line in HISTORY.read_text().splitlines():
            try:
                e = json.loads(line)
            except ValueError:
                continue
            if e.get("phase") == "focus":
                entries.append((datetime.date.fromtimestamp(e["ts"]), int(e.get("minutes", 0))))
    except OSError:
        pass
    today = datetime.date.today()
    week_start = today - datetime.timedelta(days=today.weekday())
    t_n = sum(1 for d, _ in entries if d == today)
    t_m = sum(m for d, m in entries if d == today)
    w_m = sum(m for d, m in entries if d >= week_start)
    days = {d for d, _ in entries}
    streak, day = 0, today if today in days else today - datetime.timedelta(days=1)
    while day in days:
        streak += 1
        day -= datetime.timedelta(days=1)
    return t_n, t_m, w_m, streak


def autostart_desktop():
    return (
        "[Desktop Entry]\nType=Application\nName=Foco\nComment=Pomodoro com overlay e bloqueio de sites\n"
        f"Exec={foco_binary()}\nIcon=foco\nX-GNOME-Autostart-enabled=true\nNoDisplay=false\n"
    )


class FocoSettings(Adw.ApplicationWindow):
    def __init__(self, app):
        super().__init__(application=app, title="Foco", default_width=700, default_height=780)
        self.cfg = load_config()
        self.status = None
        self._save_source = 0
        self._size_source = 0
        self._syncing = False
        self._site_rows = []
        self._preset_rows = []

        self.toasts = Adw.ToastOverlay()
        self.banner = Adw.Banner()
        self.banner.connect("button-clicked", self.on_banner_button)

        self.stack = Adw.ViewStack()
        self.stack.add_titled_with_icon(self.build_timer_page(), "timer", "Timer", "alarm-symbolic")
        self.stack.add_titled_with_icon(self.build_block_page(), "block", "Bloqueio", "action-unavailable-symbolic")
        self.stack.add_titled_with_icon(self.build_look_page(), "look", "Aparência", "applications-graphics-symbolic")
        self.stack.add_titled_with_icon(self.build_alerts_page(), "alerts", "Alertas", "preferences-system-notifications-symbolic")

        header = Adw.HeaderBar()
        switcher = Adw.ViewSwitcher(stack=self.stack, policy=Adw.ViewSwitcherPolicy.WIDE)
        header.set_title_widget(switcher)
        bottom = Adw.ViewSwitcherBar(stack=self.stack)

        body = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        body.append(self.banner)
        body.append(self.stack)
        self.stack.set_vexpand(True)

        view = Adw.ToolbarView()
        view.add_top_bar(header)
        view.set_content(body)
        view.add_bottom_bar(bottom)
        self.toasts.set_child(view)
        self.set_content(self.toasts)

        bp = Adw.Breakpoint.new(Adw.BreakpointCondition.parse("max-width: 560sp"))
        bp.add_setter(bottom, "reveal", True)
        bp.add_setter(header, "title-widget", Gtk.Label(label="Foco", css_classes=["title"]))
        self.add_breakpoint(bp)

        self.set_size_request(360, 400)
        self.refresh_status()
        self._poll = GLib.timeout_add_seconds(2, self.refresh_status)
        self.connect("close-request", self.on_close)
        # sem foco inicial num campo: senão o que se digita ao abrir cai no "Foco"
        self._focus_cleared = False
        self.connect("notify::is-active", self.on_first_active)

    def on_first_active(self, *_):
        if self.is_active() and not self._focus_cleared:
            self._focus_cleared = True
            GLib.timeout_add(60, self.clear_focus)

    def clear_focus(self):
        focus = self.get_focus()
        if isinstance(focus, Gtk.Editable):
            focus.select_region(0, 0)
        self.set_focus(None)
        return False

    # ---------- infraestrutura ----------

    def toast(self, text):
        self.toasts.add_toast(Adw.Toast(title=text, timeout=3))

    def changed(self):
        """Grava a config um instante depois da última mudança e avisa o daemon."""
        if self._syncing:
            return
        if self._save_source:
            GLib.source_remove(self._save_source)
        self._save_source = GLib.timeout_add(250, self.flush_save)

    def flush_save(self):
        self._save_source = 0
        try:
            save_config(self.cfg)
        except OSError as e:
            self.toast(f"Não consegui salvar: {e}")
            return False
        ipc("reload")
        GLib.timeout_add(300, self.refresh_status)
        return False

    def on_close(self, *_):
        if self._save_source:
            GLib.source_remove(self._save_source)
            self.flush_save()
        return False

    def refresh_status(self):
        self.status = ipc("status")
        st = self.status
        self._syncing = True
        try:
            if st is None:
                self.banner.set_title("O Foco não está rodando")
                self.banner.set_button_label("Iniciar")
                self.banner.set_revealed(True)
            elif st.get("strict_locked"):
                self.banner.set_title("Modo rígido: este foco não pode ser interrompido. Sites removidos só voltam depois dele.")
                self.banner.set_button_label(None)
                self.banner.set_revealed(True)
            else:
                self.banner.set_revealed(False)
            self.update_overlay_rows()
            self.update_block_status()
            self.update_stats()
        finally:
            self._syncing = False
        return True

    def on_banner_button(self, *_):
        try:
            Gio.Subprocess.new([foco_binary()], Gio.SubprocessFlags.NONE)
        except GLib.Error as e:
            self.toast(f"Não consegui iniciar: {e.message}")
            return
        GLib.timeout_add(900, lambda: self.refresh_status() and False)

    # ---------- construtores de linhas ----------

    def spin_row(self, title, subtitle, lo, hi, get, set_):
        row = Adw.SpinRow.new_with_range(lo, hi, 1)
        row.set_title(title)
        if subtitle:
            row.set_subtitle(subtitle)
        row.set_value(get())
        row.connect("notify::value", lambda r, _: (set_(int(r.get_value())), self.changed()))
        return row

    def switch_row(self, title, subtitle, get, set_):
        row = Adw.SwitchRow(title=title)
        if subtitle:
            row.set_subtitle(subtitle)
        row.set_active(get())
        row.connect("notify::active", lambda r, _: (set_(r.get_active()), self.changed()))
        return row

    def scale_row(self, title, subtitle, lo, hi, step, value, on_change, fmt):
        row = Adw.ActionRow(title=title)
        if subtitle:
            row.set_subtitle(subtitle)
        scale = Gtk.Scale.new_with_range(Gtk.Orientation.HORIZONTAL, lo, hi, step)
        scale.set_value(value)
        scale.set_hexpand(True)
        scale.set_size_request(230, -1)
        scale.set_valign(Gtk.Align.CENTER)
        scale.set_draw_value(True)
        scale.set_value_pos(Gtk.PositionType.RIGHT)
        scale.set_format_value_func(lambda _s, v: fmt(v))
        scale.connect("value-changed", lambda s: on_change(s.get_value()))
        row.add_suffix(scale)
        row.scale = scale
        return row

    # ---------- Timer ----------

    def build_timer_page(self):
        c = self.cfg
        page = Adw.PreferencesPage()

        stats = Adw.PreferencesGroup(title="Seu progresso")
        grid = Gtk.Grid(column_spacing=10, row_spacing=10, column_homogeneous=True, margin_bottom=6)
        self.stat_labels = []
        for i, label in enumerate(["focos hoje", "focado hoje", "nesta semana", "dias seguidos"]):
            card = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2, css_classes=["stat-card"])
            value = Gtk.Label(label="–", css_classes=["stat-value"])
            card.append(value)
            card.append(Gtk.Label(label=label, css_classes=["stat-label"]))
            grid.attach(card, i, 0, 1, 1)
            self.stat_labels.append(value)
        stats.add(grid)
        page.add(stats)

        dur = Adw.PreferencesGroup(title="Duração", description="Em minutos.")
        dur.add(self.spin_row("Foco", None, 1, 600, lambda: c["focus_min"], lambda v: c.__setitem__("focus_min", v)))
        dur.add(self.spin_row("Pausa curta", None, 1, 240, lambda: c["short_break_min"], lambda v: c.__setitem__("short_break_min", v)))
        dur.add(self.spin_row("Pausa longa", None, 1, 240, lambda: c["long_break_min"], lambda v: c.__setitem__("long_break_min", v)))
        dur.add(self.spin_row("Pausa longa a cada", "Quantos focos formam um ciclo.", 1, 12,
                              lambda: c["long_break_every"], lambda v: c.__setitem__("long_break_every", v)))
        page.add(dur)

        auto = Adw.PreferencesGroup(title="Automático")
        auto.add(self.switch_row("Começar as pausas sozinho", "Quando o foco termina, a pausa já começa a contar.",
                                 lambda: c["auto_start_breaks"], lambda v: c.__setitem__("auto_start_breaks", v)))
        auto.add(self.switch_row("Começar o foco sozinho", "Quando a pausa termina, o próximo foco já começa.",
                                 lambda: c["auto_start_focus"], lambda v: c.__setitem__("auto_start_focus", v)))
        page.add(auto)

        disc = Adw.PreferencesGroup(title="Disciplina")
        disc.add(self.switch_row("Modo rígido", "Durante o foco não dá para pausar, pular, reiniciar nem liberar sites.",
                                 lambda: c["strict_mode"], lambda v: c.__setitem__("strict_mode", v)))
        page.add(disc)
        return page

    def update_stats(self):
        n, m, w, streak = read_stats()
        for label, text in zip(self.stat_labels, [str(n), fmt_minutes(m), fmt_minutes(w), str(streak)]):
            label.set_label(text)

    # ---------- Bloqueio ----------

    def build_block_page(self):
        b = self.cfg["block"]
        page = Adw.PreferencesPage()

        status = Adw.PreferencesGroup(title="Bloqueio de sites",
                                      description="Durante o foco os sites abaixo não abrem em nenhum navegador. "
                                                  "Nas pausas tudo volta ao normal.")
        self.helper_row = Adw.ActionRow(title="Bloqueador")
        self.helper_button = Gtk.Button(valign=Gtk.Align.CENTER)
        self.helper_button.connect("clicked", self.on_helper_button)
        self.helper_row.add_suffix(self.helper_button)
        status.add(self.helper_row)
        self.now_row = Adw.ActionRow(title="Agora")
        status.add(self.now_row)
        page.add(status)

        opts = Adw.PreferencesGroup()
        opts.add(self.switch_row("Bloquear durante o foco", None, lambda: b["enabled"], lambda v: b.__setitem__("enabled", v)))
        opts.add(self.switch_row("Cortar abas já abertas", "Quando o foco começa, derruba as conexões com esses sites.",
                                 lambda: b["kill_connections"], lambda v: b.__setitem__("kill_connections", v)))
        page.add(opts)

        presets = Adw.PreferencesGroup(title="Pacotes prontos")
        for name, icon, sites in PRESETS:
            row = Adw.SwitchRow(title=name, subtitle=", ".join(sites))
            row.set_subtitle_lines(2)
            row.add_prefix(Gtk.Image(icon_name=icon))
            row.connect("notify::active", self.on_preset_toggled, sites)
            presets.add(row)
            self._preset_rows.append((row, sites))
        page.add(presets)

        self.sites_group = Adw.PreferencesGroup(title="Seus sites", description="Subdomínios www. e m. entram junto.")
        self.add_row = Adw.EntryRow(title="Adicionar site (ex.: youtube.com)", show_apply_button=True)
        self.add_row.set_input_purpose(Gtk.InputPurpose.URL)
        self.add_row.connect("apply", self.on_add_site)
        self.add_row.connect("entry-activated", self.on_add_site)
        self.sites_group.add(self.add_row)
        page.add(self.sites_group)

        self.rebuild_sites()
        return page

    def preset_sites(self):
        on = set()
        for row, sites in self._preset_rows:
            if row.get_active():
                on.update(sites)
        return on

    def rebuild_sites(self):
        sites = self.cfg["block"]["sites"]
        self._syncing = True
        try:
            for row, preset in self._preset_rows:
                row.set_active(all(s in sites for s in preset))
        finally:
            self._syncing = False
        for row in self._site_rows:
            self.sites_group.remove(row)
        self._site_rows = []
        covered = self.preset_sites()
        custom = [s for s in sites if s not in covered]
        for site in custom:
            row = Adw.ActionRow(title=site)
            remove = Gtk.Button(icon_name="user-trash-symbolic", valign=Gtk.Align.CENTER,
                                tooltip_text="Remover", css_classes=["flat"])
            remove.connect("clicked", self.on_remove_site, site)
            row.add_suffix(remove)
            self.sites_group.add(row)
            self._site_rows.append(row)
        if not custom:
            row = Adw.ActionRow(title="Nenhum site extra", css_classes=["dim-label"])
            self.sites_group.add(row)
            self._site_rows.append(row)

    def on_preset_toggled(self, row, _pspec, preset):
        if self._syncing:
            return
        sites = self.cfg["block"]["sites"]
        if row.get_active():
            sites.extend(s for s in preset if s not in sites)
        else:
            other = set()
            for r, p in self._preset_rows:
                if r is not row and r.get_active():
                    other.update(p)
            self.cfg["block"]["sites"] = [s for s in sites if s not in preset or s in other]
        self.rebuild_sites()
        self.changed()

    def on_add_site(self, entry):
        added, bad = [], []
        for part in entry.get_text().replace(",", " ").split():
            site = normalize_site(part)
            if site is None:
                bad.append(part)
            elif site not in self.cfg["block"]["sites"]:
                self.cfg["block"]["sites"].append(site)
                added.append(site)
        entry.set_text("")
        if bad:
            self.toast(f"Endereço inválido: {', '.join(bad)}")
        if added:
            self.rebuild_sites()
            self.changed()
            self.toast(f"Bloqueando {', '.join(added)}")

    def on_remove_site(self, _button, site):
        self.cfg["block"]["sites"] = [s for s in self.cfg["block"]["sites"] if s != site]
        self.rebuild_sites()
        self.changed()

    def update_block_status(self):
        st = self.status or {}
        installed = st.get("block_installed", Path("/usr/local/libexec/foco-hosts").exists())
        if installed:
            self.helper_row.set_subtitle("Instalado. Bloqueia e libera sozinho, sem pedir senha.")
            self.helper_button.set_label("Remover")
            self.helper_button.set_css_classes([])
        else:
            self.helper_row.set_subtitle("Precisa ser instalado uma vez (pede a senha de administrador).")
            self.helper_button.set_label("Instalar")
            self.helper_button.set_css_classes(["suggested-action"])
        blocked = st.get("blocked_sites")
        if not self.status:
            self.now_row.set_subtitle("O Foco não está rodando.")
        elif blocked:
            self.now_row.set_subtitle(f"{len(blocked)} sites bloqueados neste foco.")
        else:
            self.now_row.set_subtitle("Nada bloqueado. O bloqueio liga quando um foco começa.")

    def on_helper_button(self, _button):
        installed = Path("/usr/local/libexec/foco-hosts").exists()
        script = find_file("install-bloqueio.sh")
        if script is None:
            self.toast("Não achei o install-bloqueio.sh. Rode o install.sh de novo.")
            return
        self.helper_button.set_sensitive(False)
        argv = ["pkexec", str(script), "uninstall" if installed else "install"]
        try:
            proc = Gio.Subprocess.new(argv, Gio.SubprocessFlags.STDOUT_PIPE | Gio.SubprocessFlags.STDERR_PIPE)
        except GLib.Error as e:
            self.helper_button.set_sensitive(True)
            self.toast(e.message)
            return
        proc.communicate_utf8_async(None, None, self.on_helper_done, installed)

    def on_helper_done(self, proc, result, was_installed):
        try:
            _ok, _out, err = proc.communicate_utf8_finish(result)
        except GLib.Error as e:
            err = e.message
        self.helper_button.set_sensitive(True)
        if proc.get_successful():
            self.toast("Bloqueador removido" if was_installed else "Bloqueador instalado")
            ipc("reload")
        elif proc.get_exit_status() in (126, 127):
            self.toast("Cancelado")
        else:
            self.toast((err or "Falhou").strip().splitlines()[-1])
        GLib.timeout_add(400, lambda: self.refresh_status() and False)

    # ---------- Aparência ----------

    def build_look_page(self):
        a = self.cfg["appearance"]
        page = Adw.PreferencesPage()

        ov = Adw.PreferencesGroup(title="Overlay",
                                  description="Arraste para mover · roda do mouse ou a alça ↘ redimensionam · "
                                              "clique inicia/pausa · botão direito abre esta janela.")
        self.visible_row = Adw.SwitchRow(title="Mostrar overlay")
        self.visible_row.connect("notify::active", lambda r, _: self._syncing or ipc(f"overlay {'on' if r.get_active() else 'off'}"))
        ov.add(self.visible_row)
        self.lock_row = Adw.SwitchRow(title="Travar posição", subtitle="Os cliques atravessam o overlay.")
        self.lock_row.connect("notify::active", lambda r, _: self._syncing or ipc(f"lock {'on' if r.get_active() else 'off'}"))
        ov.add(self.lock_row)
        self.size_row = self.scale_row("Tamanho", None, 90, 640, 5, 190, self.on_size, lambda v: f"{int(v)} px")
        ov.add(self.size_row)
        self.output_model = Gtk.StringList()
        self.output_row = Adw.ComboRow(title="Monitor", model=self.output_model)
        self.output_row.connect("notify::selected", self.on_output)
        ov.add(self.output_row)
        ov.add(self.switch_row("Por cima de tela cheia", "Continua visível sobre vídeos e jogos em tela cheia.",
                               lambda: a["above_fullscreen"], lambda v: a.__setitem__("above_fullscreen", v)))
        page.add(ov)

        panel = Adw.PreferencesGroup(title="Painel",
                                     description="O miniaplicativo Foco fica em Configurações do COSMIC → "
                                                 "Área de trabalho → Painel → Miniaplicativos.")
        panel.add(self.switch_row("Ícone na bandeja do sistema", "Dispensável se o miniaplicativo estiver no painel.",
                                  lambda: a["tray_icon"], lambda v: a.__setitem__("tray_icon", v)))
        page.add(panel)

        tr = Adw.PreferencesGroup(title="Transparência")
        tr.add(self.scale_row("Fundo do círculo", None, 0, 100, 1, a["background_opacity"] * 100,
                              lambda v: (a.__setitem__("background_opacity", round(v / 100, 2)), self.changed()),
                              lambda v: f"{int(v)}%"))
        tr.add(self.scale_row("Sem o mouse em cima", "Opacidade do overlay inteiro quando você não está mexendo nele.",
                              15, 100, 1, a["idle_opacity"] * 100,
                              lambda v: (a.__setitem__("idle_opacity", round(v / 100, 2)), self.changed()),
                              lambda v: f"{int(v)}%"))
        page.add(tr)

        colors = Adw.PreferencesGroup(title="Cores")
        reset = Gtk.Button(label="Restaurar", css_classes=["flat"], valign=Gtk.Align.CENTER)
        colors.set_header_suffix(reset)
        self.color_buttons = {}
        for key, title in [("focus_color", "Foco"), ("short_break_color", "Pausa curta"), ("long_break_color", "Pausa longa")]:
            row = Adw.ActionRow(title=title)
            btn = Gtk.ColorDialogButton(dialog=Gtk.ColorDialog(with_alpha=False), valign=Gtk.Align.CENTER)
            rgba = Gdk.RGBA()
            rgba.parse(a[key])
            btn.set_rgba(rgba)
            btn.connect("notify::rgba", self.on_color, key)
            row.add_suffix(btn)
            row.set_activatable_widget(btn)
            colors.add(row)
            self.color_buttons[key] = btn
        reset.connect("clicked", self.on_reset_colors)
        page.add(colors)
        return page

    def on_size(self, value):
        if self._syncing:
            return
        if self._size_source:
            GLib.source_remove(self._size_source)

        def send():
            self._size_source = 0
            ipc(f"size {int(value)}")
            return False

        self._size_source = GLib.timeout_add(90, send)

    def on_output(self, row, _pspec):
        if self._syncing:
            return
        item = row.get_selected_item()
        if item is not None:
            ipc(f"output {item.get_string()}")

    def on_color(self, btn, _pspec, key):
        c = btn.get_rgba()
        self.cfg["appearance"][key] = "#{:02x}{:02x}{:02x}".format(
            round(c.red * 255), round(c.green * 255), round(c.blue * 255))
        self.changed()

    def on_reset_colors(self, _button):
        for key, value in DEFAULT_COLORS.items():
            rgba = Gdk.RGBA()
            rgba.parse(value)
            self.color_buttons[key].set_rgba(rgba)

    def update_overlay_rows(self):
        st = self.status
        for row in (self.visible_row, self.lock_row, self.size_row, self.output_row):
            row.set_sensitive(st is not None)
        if not st:
            return
        ov = st["overlay"]
        self.visible_row.set_active(ov["visible"])
        self.lock_row.set_active(ov["locked"])
        if not self._size_source:
            self.size_row.scale.set_value(ov["size"])
        outputs = st.get("outputs", [])
        current = [self.output_model.get_string(i) for i in range(self.output_model.get_n_items())]
        if current != outputs:
            self.output_model.splice(0, len(current), outputs)
        if ov["output"] in outputs:
            self.output_row.set_selected(outputs.index(ov["output"]))

    # ---------- Alertas ----------

    def build_alerts_page(self):
        al = self.cfg["alerts"]
        page = Adw.PreferencesPage()

        g = Adw.PreferencesGroup(title="Quando uma etapa termina")
        g.add(self.switch_row("Notificação", None, lambda: al["notifications"], lambda v: al.__setitem__("notifications", v)))
        g.add(self.switch_row("Som", None, lambda: al["sound"], lambda v: al.__setitem__("sound", v)))
        vol = self.scale_row("Volume", None, 0, 100, 1, al["volume"] * 100,
                             lambda v: (al.__setitem__("volume", round(v / 100, 2)), self.changed()),
                             lambda v: f"{int(v)}%")
        test = Gtk.Button(icon_name="media-playback-start-symbolic", valign=Gtk.Align.CENTER,
                          tooltip_text="Testar", css_classes=["flat"])
        test.connect("clicked", self.on_test_sound)
        vol.add_suffix(test)
        g.add(vol)
        page.add(g)

        sysg = Adw.PreferencesGroup(title="Sistema")
        auto = Adw.SwitchRow(title="Iniciar com o sistema", subtitle="Abre o Foco quando você entra na sessão.")
        auto.set_active(AUTOSTART.exists())
        auto.connect("notify::active", self.on_autostart)
        sysg.add(auto)
        keys = Adw.ActionRow(title="Atalho de teclado",
                             subtitle="Em Configurações → Teclado → Atalhos personalizados, crie um atalho com o comando "
                                      "“foco toggle” (inicia/pausa). Também existem foco skip, foco show e foco hide.")
        keys.set_subtitle_lines(0)
        sysg.add(keys)
        page.add(sysg)
        return page

    def on_test_sound(self, _button):
        vol = self.cfg["alerts"]["volume"]
        for argv in (["pw-play", f"--volume={vol:.2f}", "/usr/share/sounds/freedesktop/stereo/complete.oga"],
                     ["paplay", f"--volume={int(vol * 65536)}", "/usr/share/sounds/freedesktop/stereo/complete.oga"]):
            try:
                Gio.Subprocess.new(argv, Gio.SubprocessFlags.NONE)
                return
            except GLib.Error:
                continue

    def on_autostart(self, row, _pspec):
        try:
            if row.get_active():
                AUTOSTART.parent.mkdir(parents=True, exist_ok=True)
                AUTOSTART.write_text(autostart_desktop())
                AUTOSTART_OFF.unlink(missing_ok=True)
            else:
                AUTOSTART.unlink(missing_ok=True)
                AUTOSTART_OFF.parent.mkdir(parents=True, exist_ok=True)
                AUTOSTART_OFF.touch()
        except OSError as e:
            self.toast(f"Não consegui mudar o início automático: {e}")


PAGES = ("timer", "block", "look", "alerts")


class App(Adw.Application):
    def __init__(self):
        super().__init__(application_id="dev.foco.Settings", flags=Gio.ApplicationFlags.HANDLES_COMMAND_LINE)
        self.window = None

    def do_command_line(self, command_line):
        args = command_line.get_arguments()[1:]
        self.activate()
        page = next((a for a in args if a in PAGES), None)
        if page:
            self.window.stack.set_visible_child_name(page)
        return 0

    def do_startup(self):
        Adw.Application.do_startup(self)
        Adw.StyleManager.get_default().set_color_scheme(Adw.ColorScheme.PREFER_DARK)
        css = Gtk.CssProvider()
        css.load_from_string(CSS)
        Gtk.StyleContext.add_provider_for_display(Gdk.Display.get_default(), css, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)

    def do_activate(self):
        if self.window is None:
            self.window = FocoSettings(self)
        self.window.present()


if __name__ == "__main__":
    sys.exit(App().run(sys.argv))
