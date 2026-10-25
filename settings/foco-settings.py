#!/usr/bin/python3
"""Central do Foco (GTK4 + libadwaita): o timer, os controles e todas as configurações.

Abre com um clique no overlay (o timer flutuante), na bandeja ou pelo miniaplicativo do painel. Grava
~/.config/foco/config.json e avisa o daemon pelo socket; o que é do overlay (visível, travado,
tamanho, monitor) vai direto por comando, porque o daemon é o dono.
"""

import copy
import datetime
import json
import math
import os
import shutil
import socket
import sys
import time
from pathlib import Path

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Gsk", "4.0")
gi.require_version("Graphene", "1.0")
gi.require_version("Adw", "1")
gi.require_version("GdkPixbuf", "2.0")
from gi.repository import Adw, Gdk, GdkPixbuf, Gio, GLib, Graphene, Gsk, Gtk  # noqa: E402

HOME = Path.home()


def xdg(var, fallback):
    value = os.environ.get(var)
    return Path(value) if value else HOME / fallback


CONFIG = xdg("XDG_CONFIG_HOME", ".config") / "foco" / "config.json"
DATA = xdg("XDG_DATA_HOME", ".local/share") / "foco"
HISTORY = DATA / "history.jsonl"
AUTOSTART = xdg("XDG_CONFIG_HOME", ".config") / "autostart" / "dev.foco.Foco.desktop"
# o install.sh não recria o autostart se você desligou aqui
AUTOSTART_OFF = DATA / ".autostart-off"
HELPER = Path("/usr/local/libexec/foco-hosts")
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
        # fundo desta janela; vazio = vermelho, sem imagem
        "central_image": "",
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

PHASES = [("focus", "Foco"), ("short_break", "Pausa curta"), ("long_break", "Pausa longa")]

# página: (título, ícone)
PAGES = {
    "timer": ("Tempos e ciclo", "alarm-symbolic"),
    "block": ("Sites bloqueados", "action-unavailable-symbolic"),
    "look": ("Overlay e aparência", "preferences-desktop-appearance-symbolic"),
    "alerts": ("Alertas e sistema", "preferences-system-notifications-symbolic"),
}

CSS = """
@define-color accent_bg_color #e0362c;
@define-color accent_fg_color #ffffff;
@define-color accent_color #ff7a68;
@define-color window_bg_color #170807;
@define-color window_fg_color #fff2ec;
@define-color view_bg_color #1d0b0a;
@define-color view_fg_color #fff2ec;
@define-color headerbar_bg_color #1d0b0a;
@define-color headerbar_fg_color #fff2ec;
@define-color card_bg_color alpha(#ffffff, 0.05);
@define-color card_fg_color #fff2ec;
@define-color card_shade_color alpha(#000000, 0.28);
@define-color popover_bg_color #2a1110;
@define-color popover_fg_color #fff2ec;
@define-color dialog_bg_color #221010;
@define-color dialog_fg_color #fff2ec;

window.foco { background-color: #170807; }

.foco .hero { color: #fff6f2; }
.foco .phase-label {
  font-family: "Fira Sans"; font-weight: 700; font-size: 13px; letter-spacing: 3px; color: @foco_light;
}
.foco .clock {
  font-family: "Fira Sans"; font-weight: 600; font-size: 64px; font-feature-settings: "tnum";
  color: #fff7f3; margin: -4px 0 -2px;
}
.foco .clock.dim { color: alpha(#fff7f3, 0.6); }
.foco .state { font-size: 13px; color: alpha(#ffe9e2, 0.72); }
.foco .state image { color: @foco_light; }

.foco button.start {
  min-height: 52px; min-width: 214px; padding: 0 28px; border-radius: 999px; border: none;
  background-image: linear-gradient(to bottom, @foco_bright, @foco_deep);
  color: #ffffff; font-weight: 700; font-size: 16px;
  box-shadow: 0 10px 28px alpha(@foco_dark, 0.55), inset 0 1px alpha(#ffffff, 0.28);
}
.foco button.start:hover { filter: brightness(1.07); }
.foco button.start:active { filter: brightness(0.93); }
/* o Adwaita já apaga o que está desativado; aqui o apagado é medido para continuar legível */
.foco button.start:disabled { filter: saturate(0.35); opacity: 0.6; box-shadow: none; }

.foco button.round {
  min-width: 48px; min-height: 48px; padding: 0; border-radius: 999px;
  background-color: alpha(#1c0807, 0.58); color: #fff2ec;
  box-shadow: inset 0 0 0 1px alpha(#ffe0d4, 0.14);
}
.foco button.round:hover { background-color: alpha(#2c0f0d, 0.72); }
.foco button.round:disabled { filter: none; opacity: 1; color: alpha(#fff2ec, 0.3); background-color: alpha(#1c0807, 0.45); }

.foco .phases {
  padding: 4px; border-radius: 999px; background-color: alpha(#1c0807, 0.6);
  box-shadow: inset 0 0 0 1px alpha(#ffe0d4, 0.12);
}
.foco .phases button {
  min-height: 32px; padding: 0 16px; border-radius: 999px; background: none; box-shadow: none;
  color: alpha(#fff2ec, 0.78); font-weight: 600;
}
.foco .phases button:hover { background-color: alpha(#ffffff, 0.07); }
.foco .phases button:checked { background-color: @foco_phase; color: #ffffff; }
.foco .phases button:disabled { filter: none; opacity: 0.55; }

.foco button.chip {
  min-height: 34px; padding: 0 16px; border-radius: 999px; font-weight: 600;
  background-color: alpha(#1c0807, 0.6); color: alpha(#fff2ec, 0.8);
  box-shadow: inset 0 0 0 1px alpha(#ffe0d4, 0.14);
}
.foco button.chip:hover { background-color: alpha(#2c0f0d, 0.72); }
.foco button.chip:checked { background-color: alpha(@foco_phase, 0.28); color: #ffffff;
  box-shadow: inset 0 0 0 1px alpha(@foco_light, 0.7); }
.foco button.chip:disabled { filter: none; opacity: 0.5; }

.foco .notice {
  padding: 9px 16px; border-radius: 14px; background-color: alpha(#1c0807, 0.66);
  box-shadow: inset 0 0 0 1px alpha(@foco_phase, 0.4); color: #ffe9e2;
}

.foco .panel {
  margin: 12px 12px 12px 0; border-radius: 22px;
  background-color: alpha(#160706, 0.8);
  box-shadow: 0 12px 40px alpha(#000000, 0.38), inset 0 0 0 1px alpha(#ffd9cc, 0.08);
}
.foco .panel headerbar { background: none; box-shadow: none; min-height: 50px; }
.foco .panel scrolledwindow, .foco .panel stack, .foco .panel preferencespage { background: none; }
.foco .section {
  margin: 18px 6px 8px; font-size: 11px; font-weight: 800; letter-spacing: 1.5px; color: alpha(#ffe2d8, 0.6);
}
.foco .section.first { margin-top: 2px; }
.foco list.boxed-list { box-shadow: inset 0 0 0 1px alpha(#ffe0d4, 0.07); }
.foco .stats { padding: 14px 6px; border-radius: 14px; background-color: @card_bg_color;
  box-shadow: inset 0 0 0 1px alpha(#ffe0d4, 0.07); }
.foco .stat-value { font-family: "Fira Sans"; font-size: 21px; font-weight: 700; font-feature-settings: "tnum"; }
.foco .stat-label { font-size: 11px; color: alpha(#ffe9e2, 0.6); }
.foco .chevron { opacity: 0.45; }
.foco .thumb {
  min-width: 76px; min-height: 44px; border-radius: 9px; background-size: cover; background-position: center;
  box-shadow: inset 0 0 0 1px alpha(#ffffff, 0.14);
}
"""

RED = "radial-gradient(circle farthest-corner at 24% 20%, #d63d1f 0%, #a1201a 26%, #5d0e10 56%, #260707 84%, #170505 100%)"


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
    for base in (here.parent / "system", DATA / "system", here):
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


def rgb(hex_color):
    try:
        h = hex_color.lstrip("#")
        return tuple(int(h[i:i + 2], 16) / 255 for i in (0, 2, 4))
    except (ValueError, AttributeError):
        return (1.0, 0.42, 0.37)


def mix(a, b, t):
    return tuple(x + (y - x) * t for x, y in zip(a, b))


def hex_of(c):
    return "#{:02x}{:02x}{:02x}".format(*(round(max(0.0, min(1.0, x)) * 255) for x in c))


def import_image(src):
    """Guarda uma cópia reduzida na pasta do Foco: a original pode mudar de lugar ou ser enorme."""
    pix = GdkPixbuf.Pixbuf.new_from_file_at_scale(src, 2400, 2400, True)
    DATA.mkdir(parents=True, exist_ok=True)
    # nome novo a cada troca: o GTK não relê uma url que não mudou
    dst = DATA / f"fundo-{int(time.time())}.jpg"
    pix.savev(str(dst), "jpeg", ["quality"], ["90"])
    for old in DATA.glob("fundo-*.jpg"):
        if old != dst:
            old.unlink(missing_ok=True)
    return dst


def dynamic_css(color, image):
    c = rgb(color)
    css = (f"@define-color foco_phase {hex_of(c)};\n"
           f"@define-color foco_light {hex_of(mix(c, (1, 1, 1), 0.3))};\n"
           f"@define-color foco_dark {hex_of(mix(c, (0, 0, 0), 0.25))};\n"
           f"@define-color foco_bright {hex_of(mix(c, (1, 1, 1), 0.08))};\n"
           f"@define-color foco_deep {hex_of(mix(c, (0, 0, 0), 0.2))};\n")
    if image:
        url = f'url("{Gio.File.new_for_path(str(image)).get_uri()}")'
        # escurece um pouco para o lado do painel; o lado do timer fica com a pintura viva
        css += f"""
window.foco {{
  background-image: linear-gradient(to right, alpha(#120404, 0.06), alpha(#120404, 0.16) 48%, alpha(#120404, 0.5)), {url};
  background-size: cover, cover;
  background-position: center, 30% center;
}}
.foco .thumb {{ background-image: {url}; }}
"""
    else:
        css += f"window.foco {{ background-image: {RED}; }}\n.foco .thumb {{ background-image: {RED}; }}\n"
    return css


def section(text, first=False):
    return Gtk.Label(label=text.upper(), xalign=0, css_classes=["section", "first"] if first else ["section"])


def color(c, alpha=1.0):
    out = Gdk.RGBA()
    out.red, out.green, out.blue = c
    out.alpha = alpha
    return out


def stops(*pairs):
    out = []
    for offset, col in pairs:
        stop = Gsk.ColorStop()
        stop.offset = offset
        stop.color = col
        out.append(stop)
    return out


def point(x, y):
    return Graphene.Point().init(x, y)


def rect(x, y, w, h):
    return Graphene.Rect().init(x, y, w, h)


def circle(cx, cy, r):
    b = Gsk.PathBuilder.new()
    b.add_circle(point(cx, cy), r)
    return b.to_path()


def arc(cx, cy, r, start, sweep):
    """Arco no sentido horário, em trechos de até 90° (um arco SVG só não fecha o círculo)."""
    b = Gsk.PathBuilder.new()
    b.move_to(cx + r * math.cos(start), cy + r * math.sin(start))
    n = max(1, math.ceil(sweep / (math.pi / 2)))
    for i in range(1, n + 1):
        a = start + sweep * i / n
        b.svg_arc_to(r, r, 0, False, True, cx + r * math.cos(a), cy + r * math.sin(a))
    return b.to_path()


def round_stroke(width):
    st = Gsk.Stroke.new(width)
    st.set_line_cap(Gsk.LineCap.ROUND)
    return st


class Ring(Gtk.Widget):
    """O mostrador grande: disco escuro, trilho e o arco do tempo que falta (como o overlay)."""

    def __init__(self, size):
        super().__init__(halign=Gtk.Align.CENTER, valign=Gtk.Align.CENTER)
        self.size = size
        self.frac = 1.0
        self.state = "idle"
        self.color = rgb(DEFAULT_COLORS["focus_color"])

    def do_measure(self, _orientation, _for_size):
        return self.size, self.size, -1, -1

    def do_snapshot(self, snap):
        w, h = self.get_width(), self.get_height()
        cx, cy = w / 2, h / 2
        r = min(w, h) / 2 - 12
        col = self.color
        box = rect(cx - r, cy - r, 2 * r, 2 * r)

        disc = Gsk.RoundedRect()
        disc.init_from_rect(box, r)
        snap.append_outset_shadow(disc, color((0, 0, 0), 0.42), 0, 6, 0, 18)
        snap.push_fill(circle(cx, cy, r), Gsk.FillRule.WINDING)
        snap.append_linear_gradient(box, point(cx, cy - r), point(cx, cy + r), stops(
            (0, color(mix((0.16, 0.06, 0.055), col, 0.1), 0.68)),
            (1, color((0.05, 0.018, 0.016), 0.82))))
        snap.pop()
        snap.append_stroke(circle(cx, cy, r - 0.5), Gsk.Stroke.new(1), color((1, 0.88, 0.84), 0.1))

        ring_w = r * 0.085
        rr = r - r * 0.1 - ring_w / 2
        snap.append_stroke(circle(cx, cy, rr), Gsk.Stroke.new(ring_w), color((1, 1, 1), 0.08))

        strength = {"running": 1.0, "paused": 0.55}.get(self.state, 0.45)
        frac = 1.0 if self.state in ("idle", "off") else self.frac
        if self.state == "off" or frac <= 0.0005:
            return
        # o que já passou some no sentido horário a partir do topo
        start = -math.pi / 2 + (1 - frac) * 2 * math.pi
        path = arc(cx, cy, rr, start, frac * 2 * math.pi)
        if self.state == "running":
            snap.append_stroke(path, round_stroke(ring_w * 1.9), color(col, 0.16))
        snap.push_stroke(path, round_stroke(ring_w))
        snap.append_linear_gradient(box, point(cx - r, cy - r), point(cx + r, cy + r), stops(
            (0, color(mix(col, (1, 1, 1), 0.32), strength)),
            (0.55, color(col, strength)),
            (1, color(mix(col, (0, 0, 0), 0.1), strength))))
        snap.pop()
        if self.state != "idle":
            head = circle(cx + rr * math.cos(start), cy + rr * math.sin(start), ring_w * 0.24)
            snap.append_fill(head, Gsk.FillRule.WINDING, color((1, 1, 1), 0.92 * max(strength, 0.7)))


class Dots(Gtk.Widget):
    """As bolinhas do ciclo: focos feitos, o atual e os que faltam até a pausa longa."""

    def __init__(self):
        super().__init__(halign=Gtk.Align.CENTER)
        self.status = None

    def do_measure(self, orientation, _for_size):
        size = 160 if orientation == Gtk.Orientation.HORIZONTAL else 10
        return size, size, -1, -1

    def do_snapshot(self, snap):
        st = self.status
        if not st:
            return
        n = max(1, min(12, int(st.get("every", 4))))
        done = int(st.get("done_in_cycle", 0))
        col = rgb(st.get("color"))
        spacing, rad = 15, 3.6
        x0 = self.get_width() / 2 - (n - 1) * spacing / 2
        y = self.get_height() / 2
        for i in range(n):
            if st["phase"] == "long_break" or i < done:
                c = color(col, 0.95)
            elif st["phase"] == "focus" and i == done and st["state"] != "idle":
                c = color(col, 0.45)
            else:
                c = color((1, 1, 1), 0.18)
            snap.append_fill(circle(x0 + i * spacing, y, rad), Gsk.FillRule.WINDING, c)


class Central(Adw.ApplicationWindow):
    def __init__(self, app):
        super().__init__(application=app, title="Foco", default_width=940, default_height=640)
        self.add_css_class("foco")
        self.cfg = load_config()
        self.status = None
        self._save_source = 0
        self._size_source = 0
        self._tick = 0
        self._close_source = 0
        self._syncing = False
        self._switches = []
        self._site_rows = []
        self._preset_rows = []
        self._built = {}
        self._stats_key = None
        self._css_key = None
        # linhas das páginas que só são montadas quando você abre a página
        self.helper_row = self.now_row = self.visible_row = self.bg_row = None

        self.css = Gtk.CssProvider()
        Gtk.StyleContext.add_provider_for_display(
            Gdk.Display.get_default(), self.css, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION + 1)

        root = Gtk.Box()
        root.append(self.build_hero())
        root.append(self.build_panel())
        self.toasts = Adw.ToastOverlay(child=root)
        self.set_content(self.toasts)
        self.set_size_request(820, 560)

        keys = Gtk.EventControllerKey()
        keys.connect("key-pressed", self.on_key)
        self.add_controller(keys)
        self.connect("close-request", self.on_close)
        # sem foco inicial num campo: senão o que se digita ao abrir cai num campo
        self._focus_cleared = False
        self.connect("notify::is-active", self.on_first_active)

        self.update_summaries()
        self.refresh()

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

    def on_key(self, _ctrl, keyval, _code, _state):
        if keyval != Gdk.KEY_Escape:
            return False
        if self.stack.get_visible_child_name() != "home":
            self.go("home")
        else:
            self.close()
        return True

    # ---------- infraestrutura ----------

    def toast(self, text):
        self.toasts.add_toast(Adw.Toast(title=text, timeout=3))

    def get_cfg(self, path):
        d = self.cfg
        for k in path:
            d = d[k]
        return d

    def set_cfg(self, path, value):
        d = self.cfg
        for k in path[:-1]:
            d = d[k]
        d[path[-1]] = value

    def changed(self):
        """Grava a config um instante depois da última mudança e avisa o daemon."""
        if self._syncing:
            return
        self.update_summaries()
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
        GLib.timeout_add(300, self.refresh)
        return False

    def on_close(self, *_):
        for src in (self._tick, self._close_source, self._size_source):
            if src:
                GLib.source_remove(src)
        self._tick = self._close_source = self._size_source = 0
        if self._save_source:
            GLib.source_remove(self._save_source)
            self.flush_save()
        return False

    def apply_css(self):
        color = (self.status or {}).get("color") or self.cfg["appearance"]["focus_color"]
        image = self.image_path()
        key = (color, str(image))
        if key != self._css_key:
            self._css_key = key
            self.css.load_from_string(dynamic_css(color, image))

    def image_path(self):
        p = self.cfg["appearance"].get("central_image") or ""
        return Path(p) if p and Path(p).is_file() else None

    # ---------- estado do daemon ----------

    def refresh(self):
        if self._tick:
            GLib.source_remove(self._tick)
            self._tick = 0
        self.status = ipc("status")
        self._syncing = True
        try:
            self.apply_css()
            self.update_hero()
            self.update_quick()
            self.update_overlay_rows()
            self.update_block_status()
            self.update_stats()
        finally:
            self._syncing = False
        self._tick = GLib.timeout_add(self.next_delay(), self.on_tick)
        return False

    def on_tick(self):
        self._tick = 0
        self.refresh()
        return False

    def next_delay(self):
        st = self.status
        if st is None:
            return 2000
        if st.get("state") == "running":
            # acorda logo depois de o relógio do daemon virar o segundo
            rem = int(st.get("remaining_ms", 0)) % 1000
            return (rem or 1000) + 30
        return 1000

    def command(self, cmd):
        reply = ipc(cmd)
        if reply is None:
            self.toast("O Foco não está rodando")
        elif not reply.get("ok"):
            self.toast(reply.get("error") or "Não deu certo")
        self.refresh()
        return bool(reply and reply.get("ok"))

    def start_daemon(self):
        try:
            Gio.Subprocess.new([foco_binary()], Gio.SubprocessFlags.NONE)
        except GLib.Error as e:
            self.toast(f"Não consegui ligar o Foco: {e.message}")
            return
        self.primary.set_sensitive(False)
        GLib.timeout_add(900, self.refresh)

    # ---------- lado esquerdo: o timer ----------

    def build_hero(self):
        self.ring = Ring(304)
        self.dots = Dots()
        self.dots.set_margin_bottom(10)
        self.phase_label = Gtk.Label(css_classes=["phase-label"])
        self.clock_label = Gtk.Label(css_classes=["clock"])
        self.state_label = Gtk.Label()
        self.lock_icon = Gtk.Image(icon_name="changes-prevent-symbolic", pixel_size=12,
                                   tooltip_text="Sites bloqueados durante este foco")
        state = Gtk.Box(spacing=6, halign=Gtk.Align.CENTER, css_classes=["state"])
        state.append(self.lock_icon)
        state.append(self.state_label)
        center = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, valign=Gtk.Align.CENTER, halign=Gtk.Align.CENTER)
        for w in (self.dots, self.phase_label, self.clock_label, state):
            center.append(w)
        dial = Gtk.Overlay(child=self.ring, halign=Gtk.Align.CENTER)
        dial.add_overlay(center)

        self.reset_btn = Gtk.Button(icon_name="edit-undo-symbolic", tooltip_text="Recomeçar esta etapa",
                                    css_classes=["round"], valign=Gtk.Align.CENTER)
        self.reset_btn.connect("clicked", lambda *_: self.command("reset"))
        self.primary_icon = Gtk.Image(icon_name="media-playback-start-symbolic", pixel_size=18)
        self.primary_label = Gtk.Label(label="Iniciar foco")
        inner = Gtk.Box(spacing=10, halign=Gtk.Align.CENTER)
        inner.append(self.primary_icon)
        inner.append(self.primary_label)
        self.primary = Gtk.Button(child=inner, css_classes=["start"])
        self.primary.connect("clicked", self.on_primary)
        self.skip_btn = Gtk.Button(icon_name="media-skip-forward-symbolic", tooltip_text="Pular para a próxima etapa",
                                   css_classes=["round"], valign=Gtk.Align.CENTER)
        self.skip_btn.connect("clicked", lambda *_: self.command("skip"))
        controls = Gtk.Box(spacing=18, halign=Gtk.Align.CENTER, margin_top=28)
        for w in (self.reset_btn, self.primary, self.skip_btn):
            controls.append(w)

        self.phases = Gtk.Box(halign=Gtk.Align.CENTER, margin_top=20, spacing=2, css_classes=["phases"])
        self.phase_buttons = {}
        first = None
        for key, name in PHASES:
            b = Gtk.ToggleButton(label=name)
            if first is None:
                first = b
            else:
                b.set_group(first)
            b.connect("toggled", self.on_phase_toggled, key)
            self.phases.append(b)
            self.phase_buttons[key] = b

        self.overlay_icon = Gtk.Image(icon_name="view-reveal-symbolic")
        self.overlay_label = Gtk.Label(label="Overlay")
        inner = Gtk.Box(spacing=8)
        inner.append(self.overlay_icon)
        inner.append(self.overlay_label)
        self.overlay_btn = Gtk.ToggleButton(child=inner, halign=Gtk.Align.CENTER, margin_top=14, css_classes=["chip"],
                                            tooltip_text="Mostra ou esconde o overlay (o timer flutuante)")
        self.overlay_btn.connect("notify::active", self.on_visible)

        self.notice = Gtk.Label(wrap=True, justify=Gtk.Justification.CENTER, max_width_chars=42,
                                halign=Gtk.Align.CENTER, margin_top=18, css_classes=["notice"], visible=False)

        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, valign=Gtk.Align.CENTER, halign=Gtk.Align.CENTER,
                      margin_start=28, margin_end=28, margin_top=28, margin_bottom=28, css_classes=["hero"])
        for w in (dial, controls, self.phases, self.overlay_btn, self.notice):
            box.append(w)
        # o fundo do lado do timer também arrasta a janela
        return Gtk.WindowHandle(child=box, hexpand=True)

    def update_hero(self):
        st = self.status
        ring = self.ring
        if st is None:
            ring.state = "off"
            ring.color = rgb(self.cfg["appearance"]["focus_color"])
            self.phase_label.set_label("FOCO")
            self.clock_label.set_label(f"{self.cfg['focus_min']:02d}:00")
            self.clock_label.add_css_class("dim")
            self.state_label.set_label("o Foco está desligado")
            self.lock_icon.set_visible(False)
            self.primary_label.set_label("Ligar o Foco")
            self.primary_icon.set_from_icon_name("system-shutdown-symbolic")
            for w in (self.reset_btn, self.skip_btn, self.phases):
                w.set_sensitive(False)
            self.primary.set_sensitive(True)
            self.notice.set_visible(False)
        else:
            state, phase = st["state"], st["phase"]
            strict = st.get("strict_locked", False)
            ring.state = state
            ring.color = rgb(st.get("color"))
            total = max(1, int(st.get("total_ms", 1)))
            ring.frac = max(0.0, min(1.0, int(st.get("remaining_ms", 0)) / total))
            self.phase_label.set_label(st.get("phase_name", "").upper())
            self.clock_label.set_label(st.get("clock", ""))
            if state == "paused":
                self.clock_label.add_css_class("dim")
            else:
                self.clock_label.remove_css_class("dim")
            text = {"running": "em andamento", "paused": "pausado"}.get(state, "pronto para começar")
            self.state_label.set_label(text)
            self.lock_icon.set_visible(bool(st.get("blocking")))
            if state == "running":
                label, icon = "Pausar", "media-playback-pause-symbolic"
            elif state == "paused":
                label, icon = "Continuar", "media-playback-start-symbolic"
            else:
                label = "Iniciar foco" if phase == "focus" else "Iniciar pausa"
                icon = "media-playback-start-symbolic"
            self.primary_label.set_label(label)
            self.primary_icon.set_from_icon_name(icon)
            self.primary.set_sensitive(not strict)
            self.reset_btn.set_sensitive(not strict)
            self.skip_btn.set_sensitive(not strict)
            self.phases.set_sensitive(not strict)
            if phase in self.phase_buttons:
                self.phase_buttons[phase].set_active(True)
            self.notice.set_label("Modo rígido: pausar, pular e liberar sites só depois que este foco acabar.")
            self.notice.set_visible(strict)
        self.dots.status = st
        ring.queue_draw()
        self.dots.queue_draw()

    def on_primary(self, _button):
        st = self.status
        if st is None:
            self.start_daemon()
            return
        if st["state"] == "running":
            self.command("pause")
        elif self.command("start") and not self._close_source:
            # começou: a central sai da frente, o overlay continua mostrando o tempo
            self._close_source = GLib.timeout_add(550, self.close_after_start)

    def close_after_start(self):
        self._close_source = 0
        self.close()
        return False

    def on_phase_toggled(self, button, key):
        if self._syncing or not button.get_active():
            return
        if self.status and self.status.get("phase") != key:
            self.command(f"phase {key}")

    # ---------- lado direito: atalhos e configurações ----------

    def build_panel(self):
        self.back_btn = Gtk.Button(icon_name="go-previous-symbolic", tooltip_text="Voltar", visible=False)
        self.back_btn.connect("clicked", lambda *_: self.go("home"))
        self.title_label = Gtk.Label(label="Foco", css_classes=["title"])
        header = Adw.HeaderBar(title_widget=self.title_label, decoration_layout=":close")
        header.pack_start(self.back_btn)
        self.stack = Gtk.Stack(transition_type=Gtk.StackTransitionType.SLIDE_LEFT_RIGHT, transition_duration=220,
                               vexpand=True, vhomogeneous=False)
        self.stack.add_named(self.build_home(), "home")
        panel = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, width_request=440, css_classes=["panel"])
        panel.append(header)
        panel.append(self.stack)
        return panel

    def go(self, name):
        if name != "home" and name not in self._built:
            builder = {"timer": self.build_timer_page, "block": self.build_block_page,
                       "look": self.build_look_page, "alerts": self.build_alerts_page}[name]
            self._syncing = True
            try:
                self._built[name] = builder()
            finally:
                self._syncing = False
            self.stack.add_named(self._built[name], name)
            self.refresh()
        came_from = self.stack.get_visible_child_name()
        self.stack.set_visible_child_name(name)
        self.title_label.set_label("Foco" if name == "home" else PAGES[name][0])
        self.back_btn.set_visible(name != "home")
        # o GTK passaria o foco para o primeiro campo da página (e o que se digita cairia nele)
        if name != "home":
            self.back_btn.grab_focus()
        elif came_from in self.nav_rows:
            self.nav_rows[came_from].grab_focus()

    def build_home(self):
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, margin_start=16, margin_end=16, margin_bottom=18)

        box.append(section("Neste foco", first=True))
        quick = Gtk.ListBox(selection_mode=Gtk.SelectionMode.NONE, css_classes=["boxed-list"])
        self.block_quick = self.switch_row("Bloquear sites", None, ("block", "enabled"), icon="action-unavailable-symbolic")
        self.strict_quick = self.switch_row("Modo rígido", None, ("strict_mode",), icon="changes-prevent-symbolic")
        for row in (self.block_quick, self.strict_quick):
            quick.append(row)
        box.append(quick)

        box.append(section("Hoje"))
        stats = Gtk.Box(homogeneous=True, css_classes=["stats"])
        self.stat_labels = []
        for label in ["focos", "focado", "na semana", "dias seguidos"]:
            cell = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=1)
            value = Gtk.Label(label="–", css_classes=["stat-value"])
            cell.append(value)
            cell.append(Gtk.Label(label=label, css_classes=["stat-label"]))
            stats.append(cell)
            self.stat_labels.append(value)
        box.append(stats)

        box.append(section("Configurações"))
        nav = Gtk.ListBox(selection_mode=Gtk.SelectionMode.NONE, css_classes=["boxed-list"])
        self.nav_rows = {}
        for key, (title, icon) in PAGES.items():
            row = Adw.ActionRow(title=title, activatable=True)
            row.add_prefix(Gtk.Image(icon_name=icon))
            row.add_suffix(Gtk.Image(icon_name="go-next-symbolic", css_classes=["chevron"]))
            row.connect("activated", lambda _r, k=key: self.go(k))
            nav.append(row)
            self.nav_rows[key] = row
        box.append(nav)
        return Gtk.ScrolledWindow(child=box, hscrollbar_policy=Gtk.PolicyType.NEVER)

    def on_visible(self, row, _pspec):
        if not self._syncing:
            ipc(f"overlay {'on' if row.get_active() else 'off'}")
            self.refresh()

    def update_quick(self):
        st = self.status
        strict = bool(st and st.get("strict_locked"))
        installed = (st or {}).get("block_installed", HELPER.exists())
        n = len(self.cfg["block"]["sites"])
        if strict:
            sub = "Travado até este foco acabar"
        elif not self.cfg["block"]["enabled"]:
            sub = "Desligado: nenhum site sai do ar"
        elif n == 0:
            sub = "A lista está vazia (em Sites bloqueados)"
        elif not installed:
            sub = "Falta instalar o bloqueador (em Sites bloqueados)"
        elif st and st.get("blocking"):
            k = len(st.get("blocked_sites") or [])
            sub = f"{k} {'site fora' if k == 1 else 'sites fora'} do ar agora"
        else:
            sub = f"{n} {'site sai' if n == 1 else 'sites saem'} do ar durante o foco"
        self.block_quick.set_subtitle(sub)
        self.block_quick.set_sensitive(not strict)
        self.strict_quick.set_subtitle("Travado até este foco acabar" if strict
                                       else "Sem pausar, pular nem liberar sites até o foco acabar")
        self.strict_quick.set_sensitive(not strict)
        visible = bool(st and st["overlay"]["visible"])
        self.overlay_btn.set_sensitive(st is not None)
        self.overlay_btn.set_active(visible)
        self.overlay_icon.set_from_icon_name("view-reveal-symbolic" if visible else "view-conceal-symbolic")
        self.overlay_label.set_label("Overlay visível" if visible else "Overlay escondido")

    def update_summaries(self):
        c = self.cfg
        self.nav_rows["timer"].set_subtitle(
            f"Foco de {c['focus_min']} min · pausas de {c['short_break_min']} e {c['long_break_min']} min")
        n = len(c["block"]["sites"])
        self.nav_rows["block"].set_subtitle(f"{n} {'site' if n == 1 else 'sites'} na lista")
        self.nav_rows["look"].set_subtitle("Fundo desta janela, cores e o overlay")
        al = c["alerts"]
        alerts = " e ".join(x for x, on in (("notificação", al["notifications"]), ("som", al["sound"])) if on)
        self.nav_rows["alerts"].set_subtitle((alerts.capitalize() or "Silencioso") + " ao fim de cada etapa")

    def update_stats(self):
        today = (self.status or {}).get("today", {})
        key = (today.get("count"), today.get("minutes"), datetime.date.today())
        if key == self._stats_key:
            return
        self._stats_key = key
        n, m, w, streak = read_stats()
        for label, text in zip(self.stat_labels, [str(n), fmt_minutes(m), fmt_minutes(w), str(streak)]):
            label.set_label(text)

    # ---------- construtores de linhas ----------

    def spin_row(self, title, subtitle, lo, hi, path):
        row = Adw.SpinRow.new_with_range(lo, hi, 1)
        row.set_title(title)
        if subtitle:
            row.set_subtitle(subtitle)
        row.set_value(self.get_cfg(path))
        row.connect("notify::value", lambda r, _: (self.set_cfg(path, int(r.get_value())), self.changed()))
        return row

    def switch_row(self, title, subtitle, path, icon=None):
        """Chave ligada a um item da config; a mesma chave em outro lugar acompanha."""
        row = Adw.SwitchRow(title=title)
        if subtitle:
            row.set_subtitle(subtitle)
        if icon:
            row.add_prefix(Gtk.Image(icon_name=icon))
        row.set_active(self.get_cfg(path))
        row.connect("notify::active", self.on_switch, path)
        self._switches.append((row, path))
        return row

    def on_switch(self, row, _pspec, path):
        if self._syncing:
            return
        self.set_cfg(path, row.get_active())
        self._syncing = True
        try:
            for other, p in self._switches:
                if other is not row and p == path:
                    other.set_active(row.get_active())
        finally:
            self._syncing = False
        self.changed()

    def scale_row(self, title, subtitle, lo, hi, step, value, on_change, fmt):
        row = Adw.ActionRow(title=title)
        if subtitle:
            row.set_subtitle(subtitle)
        scale = Gtk.Scale.new_with_range(Gtk.Orientation.HORIZONTAL, lo, hi, step)
        scale.set_value(value)
        scale.set_hexpand(True)
        scale.set_size_request(190, -1)
        scale.set_valign(Gtk.Align.CENTER)
        scale.set_draw_value(True)
        scale.set_value_pos(Gtk.PositionType.RIGHT)
        scale.set_format_value_func(lambda _s, v: fmt(v))
        scale.connect("value-changed", lambda s: on_change(s.get_value()))
        row.add_suffix(scale)
        row.scale = scale
        return row

    def button_row(self, title, subtitle, label, on_click, css=None):
        row = Adw.ActionRow(title=title, subtitle=subtitle or "")
        button = Gtk.Button(label=label, valign=Gtk.Align.CENTER, css_classes=css or [])
        button.connect("clicked", on_click)
        row.add_suffix(button)
        return row

    # ---------- Tempos e ciclo ----------

    def build_timer_page(self):
        page = Adw.PreferencesPage()

        dur = Adw.PreferencesGroup(title="Duração", description="Em minutos.")
        dur.add(self.spin_row("Foco", None, 1, 600, ("focus_min",)))
        dur.add(self.spin_row("Pausa curta", None, 1, 240, ("short_break_min",)))
        dur.add(self.spin_row("Pausa longa", None, 1, 240, ("long_break_min",)))
        page.add(dur)

        cycle = Adw.PreferencesGroup(title="Ciclo")
        cycle.add(self.spin_row("Pausa longa a cada", "Quantos focos formam um ciclo.", 1, 12, ("long_break_every",)))
        cycle.add(self.button_row("Zerar o ciclo", "Volta para o primeiro foco.", "Zerar",
                                  lambda *_: self.command("reset-cycle")))
        page.add(cycle)

        auto = Adw.PreferencesGroup(title="Automático")
        auto.add(self.switch_row("Começar as pausas sozinho", "Quando o foco termina, a pausa já começa a contar.",
                                 ("auto_start_breaks",)))
        auto.add(self.switch_row("Começar o foco sozinho", "Quando a pausa termina, o próximo foco já começa.",
                                 ("auto_start_focus",)))
        page.add(auto)
        return page

    # ---------- Sites bloqueados ----------

    def build_block_page(self):
        page = Adw.PreferencesPage()

        status = Adw.PreferencesGroup(description="Durante o foco os sites da lista não abrem em nenhum navegador. "
                                                  "Nas pausas tudo volta ao normal.")
        self.helper_row = Adw.ActionRow(title="Bloqueador")
        self.helper_button = Gtk.Button(valign=Gtk.Align.CENTER)
        self.helper_button.connect("clicked", self.on_helper_button)
        self.helper_row.add_suffix(self.helper_button)
        status.add(self.helper_row)
        self.now_row = Adw.ActionRow(title="Agora")
        status.add(self.now_row)
        status.add(self.switch_row("Bloquear durante o foco", None, ("block", "enabled")))
        status.add(self.switch_row("Cortar abas já abertas", "Quando o foco começa, derruba as conexões com esses sites.",
                                   ("block", "kill_connections")))
        page.add(status)

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
        was = self._syncing
        self._syncing = True
        try:
            for row, preset in self._preset_rows:
                row.set_active(all(s in sites for s in preset))
        finally:
            self._syncing = was
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
        if self.status and self.status.get("strict_locked"):
            self.toast(f"{site} só volta a abrir quando este foco acabar")

    def update_block_status(self):
        if self.helper_row is None:
            return
        st = self.status or {}
        installed = st.get("block_installed", HELPER.exists())
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
            self.now_row.set_subtitle("O Foco está desligado.")
        elif blocked:
            self.now_row.set_subtitle(f"{len(blocked)} sites bloqueados neste foco.")
        else:
            self.now_row.set_subtitle("Nada bloqueado. O bloqueio liga quando um foco começa.")

    def on_helper_button(self, _button):
        installed = HELPER.exists()
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
        GLib.timeout_add(400, self.refresh)

    # ---------- Overlay e aparência ----------

    def build_look_page(self):
        a = self.cfg["appearance"]
        page = Adw.PreferencesPage()

        central = Adw.PreferencesGroup(title="Fundo desta janela")
        self.bg_row = Adw.ActionRow(title="Imagem de fundo")
        self.bg_row.add_prefix(Gtk.Box(css_classes=["thumb"], valign=Gtk.Align.CENTER))
        self.bg_remove = Gtk.Button(icon_name="user-trash-symbolic", valign=Gtk.Align.CENTER, css_classes=["flat"],
                                    tooltip_text="Tirar a imagem e usar o fundo vermelho")
        self.bg_remove.connect("clicked", self.on_remove_image)
        choose = Gtk.Button(label="Escolher…", valign=Gtk.Align.CENTER)
        choose.connect("clicked", self.on_choose_image)
        self.bg_row.add_suffix(self.bg_remove)
        self.bg_row.add_suffix(choose)
        central.add(self.bg_row)
        page.add(central)
        self.update_bg_row()

        ov = Adw.PreferencesGroup(title="Overlay",
                                  description="Arraste para mover · roda do mouse ou a alça ↘ redimensionam · "
                                              "clique abre esta janela.")
        self.visible_row = Adw.SwitchRow(title="Mostrar o overlay")
        self.visible_row.connect("notify::active", self.on_visible)
        ov.add(self.visible_row)
        self.lock_row = Adw.SwitchRow(title="Travar posição",
                                      subtitle="Os cliques atravessam o overlay; abra esta janela pela bandeja.")
        self.lock_row.connect("notify::active", lambda r, _: self._syncing or ipc(f"lock {'on' if r.get_active() else 'off'}"))
        ov.add(self.lock_row)
        self.size_row = self.scale_row("Tamanho", None, 90, 640, 5, 190, self.on_size, lambda v: f"{int(v)} px")
        ov.add(self.size_row)
        self.output_model = Gtk.StringList()
        self.output_row = Adw.ComboRow(title="Monitor", model=self.output_model)
        self.output_row.connect("notify::selected", self.on_output)
        ov.add(self.output_row)
        ov.add(self.switch_row("Por cima de tela cheia", "Continua visível sobre vídeos e jogos em tela cheia.",
                               ("appearance", "above_fullscreen")))
        page.add(ov)

        tr = Adw.PreferencesGroup(title="Transparência do overlay")
        tr.add(self.scale_row("Fundo", None, 0, 100, 1, a["background_opacity"] * 100,
                              lambda v: (a.__setitem__("background_opacity", round(v / 100, 2)), self.changed()),
                              lambda v: f"{int(v)}%"))
        tr.add(self.scale_row("Sem o mouse em cima", "Opacidade do overlay inteiro quando você não está mexendo nele.",
                              15, 100, 1, a["idle_opacity"] * 100,
                              lambda v: (a.__setitem__("idle_opacity", round(v / 100, 2)), self.changed()),
                              lambda v: f"{int(v)}%"))
        page.add(tr)

        colors = Adw.PreferencesGroup(title="Cores das etapas")
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

        panel = Adw.PreferencesGroup(title="Painel",
                                     description="O miniaplicativo Foco fica em Configurações do COSMIC → "
                                                 "Área de trabalho → Painel → Miniaplicativos.")
        panel.add(self.switch_row("Ícone na bandeja do sistema", "Dispensável com o miniaplicativo no painel.",
                                  ("appearance", "tray_icon")))
        page.add(panel)
        return page

    def update_bg_row(self):
        has = self.image_path() is not None
        self.bg_row.set_subtitle("Sua imagem" if has else "Vermelho, sem imagem")
        self.bg_remove.set_visible(has)

    def on_choose_image(self, _button):
        images = Gtk.FileFilter(name="Imagens")
        images.add_mime_type("image/*")
        filters = Gio.ListStore.new(Gtk.FileFilter)
        filters.append(images)
        dialog = Gtk.FileDialog(title="Imagem de fundo", modal=True, filters=filters, default_filter=images)
        dialog.open(self, None, self.on_image_chosen)

    def on_image_chosen(self, dialog, result):
        try:
            file = dialog.open_finish(result)
        except GLib.Error:
            return  # cancelado
        try:
            dst = import_image(file.get_path())
        except (GLib.Error, OSError, TypeError) as e:
            self.toast(f"Não consegui usar essa imagem: {getattr(e, 'message', e)}")
            return
        self.cfg["appearance"]["central_image"] = str(dst)
        self.changed()
        self.apply_css()
        self.update_bg_row()

    def on_remove_image(self, _button):
        old = self.image_path()
        self.cfg["appearance"]["central_image"] = ""
        if old is not None and old.parent == DATA:
            old.unlink(missing_ok=True)
        self.changed()
        self.apply_css()
        self.update_bg_row()

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
        if self.visible_row is None:
            return
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

    # ---------- Alertas e sistema ----------

    def build_alerts_page(self):
        al = self.cfg["alerts"]
        page = Adw.PreferencesPage()

        g = Adw.PreferencesGroup(title="Quando uma etapa termina")
        g.add(self.switch_row("Notificação", None, ("alerts", "notifications")))
        g.add(self.switch_row("Som", None, ("alerts", "sound")))
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
        auto = Adw.SwitchRow(title="Iniciar com o sistema", subtitle="Liga o Foco quando você entra na sessão.")
        auto.set_active(AUTOSTART.exists())
        auto.connect("notify::active", self.on_autostart)
        sysg.add(auto)
        keys = Adw.ActionRow(title="Atalho de teclado",
                             subtitle="Em Configurações → Teclado → Atalhos personalizados, crie um atalho com o comando "
                                      "“foco toggle” (inicia/pausa). Também existem foco skip, foco show e foco hide.")
        keys.set_subtitle_lines(0)
        sysg.add(keys)
        sysg.add(self.button_row("Desligar o Foco", "Fecha o overlay e para o timer até você ligar de novo.",
                                 "Desligar", self.on_quit, css=["destructive-action"]))
        page.add(sysg)
        return page

    def on_quit(self, _button):
        if ipc("quit") is None:
            self.toast("O Foco já está desligado")
        GLib.timeout_add(400, self.refresh)

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


class App(Adw.Application):
    def __init__(self):
        super().__init__(application_id="dev.foco.Settings", flags=Gio.ApplicationFlags.HANDLES_COMMAND_LINE)
        self.window = None

    def do_command_line(self, command_line):
        args = command_line.get_arguments()[1:]
        # clique no overlay ou na bandeja com a central já na frente: fecha, como um menu
        if "toggle" in args and self.window is not None and self.window.is_active():
            self.window.close()
            return 0
        self.activate()
        page = next((a for a in args if a in PAGES), None)
        if page:
            self.window.go(page)
        return 0

    def do_startup(self):
        Adw.Application.do_startup(self)
        Adw.StyleManager.get_default().set_color_scheme(Adw.ColorScheme.FORCE_DARK)
        css = Gtk.CssProvider()
        css.load_from_string(CSS)
        Gtk.StyleContext.add_provider_for_display(Gdk.Display.get_default(), css, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)

    def do_activate(self):
        if self.window is None:
            self.window = Central(self)
            self.window.connect("destroy", self.on_window_destroy)
        self.window.present()

    def on_window_destroy(self, _window):
        self.window = None


if __name__ == "__main__":
    sys.exit(App().run(sys.argv))
