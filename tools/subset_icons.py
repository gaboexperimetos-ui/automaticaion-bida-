#!/usr/bin/env python3
"""Subsetea Material Symbols a los iconos que usa la app.

Recolecta las ligaduras `material-symbols-outlined` de `static/` y `src/`,
poda el GSUB para conservar solo esos glifos y escribe el woff2 servido en
`static/fonts/`. El font completo vive en `tools/fonts/` y no se sirve.

Uso: `python3 tools/subset_icons.py` (requiere fonttools con brotli).
"""
import pathlib
import re
import sys

from fontTools import subset
from fontTools.ttLib import TTFont

ROOT = pathlib.Path(__file__).resolve().parents[1]
FULL = ROOT / "tools/fonts/material-symbols-outlined.full.woff2"
OUT = ROOT / "static/fonts/material-symbols-outlined.woff2"

# Nombres que el shell construye en tiempo de ejecucion y no son literales
# junto a la clase (icon() con ternarios y valores por defecto).
DYNAMIC = {
    "arrow_right",
    "chevron_right",
    "check_circle",
    "error",
    "info",
}


def icon_names():
    names = set(DYNAMIC)
    for root in (ROOT / "static", ROOT / "src"):
        for path in root.rglob("*"):
            if path.suffix not in {".html", ".js", ".zig"}:
                continue
            try:
                text = path.read_text(errors="ignore")
            except OSError:
                continue
            names.update(
                m.group(1)
                for m in re.finditer(
                    r"material-symbols-outlined[^>]*>\s*([a-z][a-z0-9_]*)", text
                )
            )
            names.update(
                m.group(1)
                for m in re.finditer(
                    r'material-symbols-outlined"\s*,\s*"([a-z][a-z0-9_]*)"', text
                )
            )
            names.update(
                m.group(1)
                for m in re.finditer(r'icon\("([a-z][a-z0-9_]*)"\)', text)
            )
    return names


def prune_ligatures(font, keep):
    """Elimina del GSUB las ligaduras cuyo glifo destino no se usa."""
    components = set()
    for lookup in font["GSUB"].table.LookupList.Lookup:
        for ext in lookup.SubTable:
            table = getattr(ext, "ExtSubTable", ext)
            ligatures = getattr(table, "ligatures", None)
            if not ligatures:
                continue
            for first in list(ligatures):
                kept = [l for l in ligatures[first] if l.LigGlyph in keep]
                if kept:
                    ligatures[first] = kept
                    components.add(first)
                    for lig in kept:
                        components.update(lig.Component)
                else:
                    del ligatures[first]
    return components


def main():
    font = TTFont(FULL)
    glyphs = set(font.getGlyphOrder())
    keep = {name for name in icon_names() if name in glyphs}
    components = prune_ligatures(font, keep)

    options = subset.Options()
    options.flavor = "woff2"
    options.layout_features = ["rlig"]
    options.hinting = False
    subsetter = subset.Subsetter(options=options)
    subsetter.populate(glyphs=sorted(keep | components))
    subsetter.subset(font)
    font.flavor = "woff2"
    font.save(OUT)
    print(f"{len(keep)} iconos · {OUT.stat().st_size} B -> {OUT.relative_to(ROOT)}")


if __name__ == "__main__":
    sys.exit(main())
