#!/usr/bin/env python3
"""Genera `static/app/nova-<grupo>.css` desde `static/app/nova.css`.

Cada variante concatena, en orden de cascada, tailwind.css + polish.css +
el grupo de nova.css + el tema nebula + music.css, para que la pagina cargue
un unico CSS.

Cada variante conserva el orden original de las reglas e incluye:
  - las reglas sin clases (base, variables, reset),
  - las que usan clases presentes en el HTML de las rutas del grupo,
  - las que usan clases que crean los scripts del shell en tiempo de ejecucion.

El servidor arranca con `python3 tools/gen_nova_groups.py` (o `zig build`
si el paso esta cableado) y el shell enlaza la variante segun la ruta:
`/static/app/nova-<grupo>.css`.

Uso: `python3 tools/gen_nova_groups.py` con el servidor en marcha.
"""
import json
import pathlib
import re
import sys
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[1]
CSS = (ROOT / "static/app/nova.css").read_text()
OUT = ROOT / "static/app"
EXTRA = ["polish.css", "tailwind.css", "music.css"]
BASE_URL = "http://127.0.0.1:3100"

GROUPS = {
    "home": ["/dashboard"],
    "notes": ["/dashboard/notes"],
    "calendar": ["/dashboard/calendar"],
    "tasks": ["/dashboard/tasks"],
    "audio": ["/dashboard/audio"],
    "settings": ["/dashboard/settings"],
    "search": ["/dashboard/search"],
    "material": ["/dashboard/material"],
    "flashcards": ["/dashboard/flashcards"],
    "academic": [
        "/dashboard/subjects", "/dashboard/vida-escolar", "/dashboard/horario",
        "/dashboard/planner", "/dashboard/examenes", "/dashboard/mensajes",
        "/dashboard/perfil", "/dashboard/classroom", "/dashboard/google-calendar",
        "/dashboard/opencode",
    ],
    "chat": ["/dashboard/chat", "/dashboard/ai"],
    "office": ["/office"],
}


def parse_items(css):
    """Items de primer nivel: (tipo, texto, selector)."""
    items = []
    i, n = 0, len(css)
    pending = ""
    while i < n:
        if css[i].isspace():
            i += 1
            continue
        if css.startswith("/*", i):
            j = css.find("*/", i + 2)
            j = n if j < 0 else j + 2
            pending += css[i:j]
            i = j
            continue
        j = i
        depth = 0
        while j < n:
            c = css[j]
            if c == "{":
                depth += 1
            elif c == "}":
                depth -= 1
                if depth <= 0:
                    j += 1
                    break
            elif c == ";" and depth == 0:
                j += 1
                break
            j += 1
        end = min(j, n)
        raw = pending + css[i:end]
        pending = ""
        sel = raw.split("{", 1)[0].strip()
        items.append(("at" if sel.startswith("@") else "rule", raw, sel))
        i = end
    return items


def classes_in(text):
    return set(re.findall(r"\.([A-Za-z_][\w-]*)", text))


def dynamic_classes():
    """Clases que los scripts ponen en el DOM y no estan en el HTML servido."""
    out = set()
    for path in (ROOT / "static/app").glob("*.js"):
        text = path.read_text(errors="ignore")
        for m in re.finditer(r"class=\\?\"([^\"\\]+)", text):
            out.update(m.group(1).split())
        for m in re.finditer(r"el\(\"[a-z0-9]+\",\s*\"([^\"]+)\"", text):
            out.update(m.group(1).split())
        for m in re.finditer(r"className\s*=\s*\"([^\"]+)\"", text):
            out.update(m.group(1).split())
    return out


def route_classes(url):
    with urllib.request.urlopen(BASE_URL + url, timeout=10) as r:
        html = r.read().decode("utf-8", "ignore")
    out = set()
    for m in re.finditer(r"class=\"([^\"]*)\"", html):
        out.update(m.group(1).split())
    return out


def keep(sel, classes, dynamic):
    cs = classes_in(sel)
    return not cs or bool(cs & (classes | dynamic))


def filter_items(items, classes, dynamic):
    out = []
    for kind, raw, sel in items:
        if kind == "at":
            open_at = raw.find("{")
            if open_at < 0:
                out.append(raw)
                continue
            body = raw[open_at + 1 : raw.rfind("}")]
            kept = filter_items(parse_items(body), classes, dynamic)
            if kept:
                out.append(raw[: open_at + 1] + "\n" + "\n".join(kept) + "\n}")
        elif keep(sel, classes, dynamic):
            out.append(raw)
    return out


def theme_keep(sel):
    """Conserva lo que aplica al tema nebula y lo comun a todos los temas."""
    s = sel.strip()
    if ":not([data-theme=" in s:
        return False
    if '[data-theme="' in s:
        return '[data-theme="nebula"]' in s
    return True


def filter_theme(items):
    out = []
    for kind, raw, sel in items:
        if kind == "at":
            open_at = raw.find("{")
            if open_at < 0:
                out.append(raw)
                continue
            body = raw[open_at + 1 : raw.rfind("}")]
            kept = filter_theme(parse_items(body))
            if kept:
                out.append(raw[: open_at + 1] + "\n" + "\n".join(kept) + "\n}")
        elif theme_keep(sel):
            out.append(raw)
    return out


def theme_css():
    """themes.css sin los bloques de otros temas.

    Conserva el bloque de tokens nebula, las reglas comunes a `[data-theme]`
    y los overrides propios de nebula; descarta el resto de paletas y las
    reglas `:not([data-theme="nebula"])`.
    """
    css = (OUT / "themes.css").read_text()
    return "\n".join(filter_theme(parse_items(css)))


def main():
    items = parse_items(CSS)
    dynamic = dynamic_classes()
    head_css = (ROOT / "static/fonts.css").read_text() + "\n" + "\n".join(
        (OUT / name).read_text() for name in ("tailwind.css", "polish.css")
    )
    tail_css = (OUT / "music.css").read_text()
    theme = theme_css()
    total = 0
    for group, urls in GROUPS.items():
        classes = set()
        for u in urls:
            classes |= route_classes(u)
        nova = "\n".join(filter_items(items, classes, dynamic))
        # Mismo orden de cascada que el shell clasico:
        # tailwind -> polish -> nova -> tema -> music.
        body = "\n".join((head_css, nova, theme, tail_css))
        (OUT / f"nova-{group}.css").write_text(body)
        total += 1
        print(f"{group:10s} {len(body):7d}B  clases={len(classes)}")
    print(f"{total} variantes · base {len(CSS)}B")


if __name__ == "__main__":
    sys.exit(main())