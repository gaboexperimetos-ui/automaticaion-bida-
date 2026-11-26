#!/usr/bin/env bash
# Instala el lanzador de escritorio de NOVA en el menú de aplicaciones del usuario.
# No toca el servicio: si study-platform no está activo, la app mostrará el error
# de carga de WebKit y podrás arrancarlo con systemctl --user start study-platform.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_DIR="$HOME/.local/bin"
APPS_DIR="$HOME/.local/share/applications"
ICON_DIR="$HOME/.local/share/icons/hicolor/scalable/apps"

zig_bin=""
if [ -x "$HOME/.cache/zig/zig-0.14.1/zig" ]; then
  zig_bin="$HOME/.cache/zig/zig-0.14.1/zig"
elif command -v zig >/dev/null 2>&1; then
  zig_bin="$(command -v zig)"
fi

if [ -n "$zig_bin" ]; then
  echo "Compilando lanzador con $zig_bin ..."
  ( cd "$ROOT" && "$zig_bin" build desktop )
fi

if [ ! -x "$ROOT/zig-out/bin/nova-desktop" ]; then
  echo "No existe $ROOT/zig-out/bin/nova-desktop." >&2
  echo "Compílalo con: zig build desktop" >&2
  exit 1
fi

mkdir -p "$BIN_DIR" "$APPS_DIR" "$ICON_DIR"
install -m 755 "$ROOT/zig-out/bin/nova-desktop" "$BIN_DIR/nova-desktop"
install -m 644 "$ROOT/static/app/favicon.svg" "$ICON_DIR/nova.svg"

cat > "$APPS_DIR/nova.desktop" <<EOF
[Desktop Entry]
Type=Application
Version=1.0
Name=NOVA · Study Platform
GenericName=Espacio académico
Comment=Notas, horario, calendario y estudio con IA
Exec=$BIN_DIR/nova-desktop
TryExec=$BIN_DIR/nova-desktop
Icon=nova
Terminal=false
Categories=Education;
StartupNotify=true
StartupWMClass=nova
Keywords=estudio;notas;calendario;horario;temario;audio;
EOF
chmod 644 "$APPS_DIR/nova.desktop"

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$APPS_DIR" >/dev/null 2>&1 || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -f -t "$HOME/.local/share/icons/hicolor" >/dev/null 2>&1 || true
fi

echo "Instalado:"
echo "  $BIN_DIR/nova-desktop"
echo "  $APPS_DIR/nova.desktop"
echo "  $ICON_DIR/nova.svg"
echo
echo "Ábrela desde el menú como «NOVA · Study Platform»."
echo "Atajos: F5 recargar · Ctrl+H inicio · F12 devtools · Ctrl+Q cerrar."
