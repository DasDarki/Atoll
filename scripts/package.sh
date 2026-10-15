#!/usr/bin/env bash
# Schnuert aus einem Release-Build das Archiv, das im GitHub-Release landet und aus dem das
# AUR-Paket atoll-bin besteht. Das Archiv traegt genau den Baum, der spaeter installiert wird.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

APP_ID="dev.dasdarki.Atoll"
ICON_SIZES=(16 22 24 32 48 64 96 128 256 512)
CEF_RUNTIME=(
  libcef.so libEGL.so libGLESv2.so libvk_swiftshader.so libvulkan.so.1 vk_swiftshader_icd.json
  chrome_100_percent.pak chrome_200_percent.pak resources.pak icudtl.dat v8_context_snapshot.bin
  chrome-sandbox
)

die() { printf '==> %s\n' "$1" >&2; exit 1; }
log() { printf '==> %s\n' "$1"; }

usage() {
  cat <<'EOF'
Ein Release-Archiv schnueren.

  scripts/package.sh [--version X.Y.Z] [--out VERZEICHNIS] [--no-build]

  --version   Die Version, die im Dateinamen steht. Ohne Angabe die aus Cargo.toml.
  --out       Wohin das Archiv kommt, in der Voreinstellung target/release-package.
  --no-build  Nicht neu bauen, nur schnueren.
EOF
}

VERSION=""
OUT=""
BUILD=1
while [[ $# -gt 0 ]]; do
  case "$1" in
    --version) VERSION="${2:-}"; shift 2 ;;
    --out) OUT="${2:-}"; shift 2 ;;
    --no-build) BUILD=0; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "Unbekannte Option: $1" ;;
  esac
done

command -v cargo >/dev/null || die "cargo fehlt."
command -v magick >/dev/null || die "imagemagick fehlt."
command -v bsdtar >/dev/null || command -v tar >/dev/null || die "tar fehlt."

[[ -n "$VERSION" ]] || VERSION="$(cargo pkgid -p atoll | sed 's/.*[#@]//')"
[[ -n "$OUT" ]] || OUT="$ROOT/target/release-package"

cef_directory() {
  local cef_version
  cef_version="$(sed -n '/^name = "cef-dll-sys"$/{n;s/^version = ".*+\(.*\)"$/\1/p}' Cargo.lock)"
  [[ -n "$cef_version" ]] || die "Die CEF-Version steht nicht in Cargo.lock."
  local directory="${CEF_PATH:-$ROOT/.cef}/$cef_version/cef_linux_x86_64"
  [[ "$directory" = /* ]] || directory="$ROOT/$directory"
  [[ -f "$directory/libcef.so" ]] || die "CEF fehlt unter $directory."
  printf '%s' "$directory"
}

if [[ $BUILD -eq 1 ]]; then
  log "Baue Atoll $VERSION (Release)."
  cargo build --release --locked -p atoll
fi

CEF="$(cef_directory)"
STAGE="$OUT/atoll-$VERSION-x86_64"
rm -rf "$STAGE"
mkdir -p "$STAGE/opt/Atoll" "$STAGE/usr/bin" "$STAGE/usr/share/applications"

log "Sammle das Programm und die CEF-Laufzeit."
install -Dm755 "$ROOT/target/release/atoll" "$STAGE/opt/Atoll/atoll"
for file in "${CEF_RUNTIME[@]}"; do
  install -Dm644 "$CEF/$file" "$STAGE/opt/Atoll/$file"
done
chmod 755 "$STAGE/opt/Atoll"/*.so* "$STAGE/opt/Atoll/chrome-sandbox"
cp -r "$CEF/locales" "$STAGE/opt/Atoll/locales"
install -Dm644 "$CEF/CREDITS.html" "$STAGE/share/CEF-CREDITS.html"
install -Dm644 "$ROOT/README.md" "$STAGE/share/README.md"

cat >"$STAGE/usr/bin/atoll" <<EOF
#!/bin/sh
exec /opt/Atoll/atoll "\$@"
EOF
chmod 755 "$STAGE/usr/bin/atoll"

cat >"$STAGE/usr/share/applications/$APP_ID.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Atoll
GenericName=Webbrowser
GenericName[en]=Web Browser
Comment=Browser mit Inseln, Workspaces und Erweiterungen
Comment[en]=Browser with islands, workspaces and extensions
Exec=atoll %U
Icon=$APP_ID
Terminal=false
Categories=Network;WebBrowser;
MimeType=text/html;x-scheme-handler/http;x-scheme-handler/https;
Keywords=browser;web;internet;
StartupNotify=true
StartupWMClass=$APP_ID
EOF

log "Rendere die Icons."
for size in "${ICON_SIZES[@]}"; do
  target="$STAGE/usr/share/icons/hicolor/${size}x${size}/apps/$APP_ID.png"
  mkdir -p "$(dirname "$target")"
  magick "$ROOT/assets/branding/logo.png" -resize "${size}x${size}" -strip "$target"
done

ARCHIVE="$OUT/atoll-$VERSION-x86_64.tar.zst"
log "Schnuere $ARCHIVE."
rm -f "$ARCHIVE"
tar --zstd -C "$STAGE" -cf "$ARCHIVE" .
rm -rf "$STAGE"
(cd "$OUT" && sha256sum "$(basename "$ARCHIVE")" >"$(basename "$ARCHIVE").sha256")

log "Fertig:"
ls -l "$ARCHIVE" "$ARCHIVE.sha256"
cat "$ARCHIVE.sha256"
