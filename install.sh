#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT"

PKGNAME="atoll"
APP_ID="dev.dasdarki.Atoll"
INSTALL_DIR="/opt/Atoll"
PACKAGE_DIR="$ROOT/target/package"
ICON_SIZES=(16 22 24 32 48 64 96 128 256 512)
CEF_RUNTIME=(
  libcef.so libEGL.so libGLESv2.so libvk_swiftshader.so libvulkan.so.1 vk_swiftshader_icd.json
  chrome_100_percent.pak chrome_200_percent.pak resources.pak icudtl.dat v8_context_snapshot.bin
  chrome-sandbox
)

usage() {
  cat <<'EOF'
Atoll lokal installieren (CachyOS / Arch)

  ./install.sh              Bauen und installieren (ueberschreibt eine vorhandene Installation)
  ./install.sh --no-build   Nur das zuletzt gebaute Paket installieren
  ./install.sh --package    Nur bauen und das Paket schnueren, nichts installieren
  ./install.sh --run        Nach der Installation direkt starten
  ./install.sh --uninstall  Atoll wieder entfernen
  ./install.sh --help       Diese Hilfe

Das Paket landet als echtes pacman-Paket im System, inklusive Desktop-Eintrag
und Icons. Deinstallieren geht auch jederzeit mit: sudo pacman -R atoll
Benutzerdaten unter ~/.local/share/atoll bleiben dabei erhalten.
EOF
}

log() { printf '\033[1;34m==>\033[0m %s\n' "$1"; }
warn() { printf '\033[1;33m==>\033[0m %s\n' "$1"; }
die() { printf '\033[1;31m==>\033[0m %s\n' "$1" >&2; exit 1; }

require() {
  command -v "$1" >/dev/null 2>&1 || die "'$1' wird gebraucht, ist aber nicht installiert${2:+ (Paket: $2)}."
}

stop_running() {
  if pgrep -f "^$INSTALL_DIR/atoll" >/dev/null 2>&1; then
    warn "Atoll laeuft noch, wird beendet."
    pkill -f "^$INSTALL_DIR/atoll" || true
    for _ in $(seq 1 20); do
      pgrep -f "^$INSTALL_DIR/atoll" >/dev/null 2>&1 || break
      sleep 0.25
    done
    pkill -9 -f "^$INSTALL_DIR/atoll" 2>/dev/null || true
  fi
}

uninstall() {
  if ! pacman -Qi "$PKGNAME" >/dev/null 2>&1; then
    log "Atoll ist nicht installiert."
    return 0
  fi
  stop_running
  log "Entferne Atoll."
  sudo pacman -R --noconfirm "$PKGNAME"
  log "Fertig. Benutzerdaten unter ~/.local/share/atoll wurden nicht angeruehrt."
}

package_version() {
  local base
  base="$(cargo pkgid -p atoll | sed 's/.*[#@]//')"
  if git -C "$ROOT" rev-parse --git-dir >/dev/null 2>&1; then
    printf '%s.r%s.g%s' "$base" "$(git -C "$ROOT" rev-list --count HEAD)" "$(git -C "$ROOT" rev-parse --short HEAD)"
  else
    printf '%s' "$base"
  fi
}

cef_directory() {
  local version
  version="$(sed -n '/^name = "cef-dll-sys"$/{n;s/^version = ".*+\(.*\)"$/\1/p}' Cargo.lock)"
  [[ -n "$version" ]] || die "Die CEF-Version steht nicht in Cargo.lock."
  local directory="$ROOT/.cef/$version/cef_linux_x86_64"
  [[ -f "$directory/libcef.so" ]] || die "CEF fehlt unter $directory (einmal 'cargo build -p atoll' laufen lassen)."
  printf '%s' "$directory"
}

render_icons() {
  local source="$ROOT/assets/branding/logo.png"
  local target="$PACKAGE_DIR/icons"
  [[ -f "$source" ]] || die "Logo nicht gefunden: $source"
  rm -rf "$target"
  mkdir -p "$target"
  for size in "${ICON_SIZES[@]}"; do
    magick "$source" -resize "${size}x${size}" -strip "$target/$size.png"
  done
}

write_package_files() {
  local version="$1" cef="$2"
  cat >"$PACKAGE_DIR/atoll" <<EOF
#!/bin/sh
exec $INSTALL_DIR/atoll "\$@"
EOF
  cat >"$PACKAGE_DIR/$APP_ID.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Atoll
GenericName=Webbrowser
GenericName[en]=Web Browser
Comment=Browser mit Inseln, Workspaces und Erweiterungen
Comment[en]=Browser with islands, workspaces and extensions
Exec=atoll
Icon=$APP_ID
Terminal=false
Categories=Network;WebBrowser;
Keywords=browser;web;internet;
StartupNotify=true
StartupWMClass=$APP_ID
EOF
  cat >"$PACKAGE_DIR/PKGBUILD" <<EOF
pkgname=$PKGNAME
pkgver=$version
pkgrel=1
pkgdesc='Browser mit Inseln, Workspaces und Erweiterungen'
arch=('x86_64')
license=('custom')
depends=('alsa-lib' 'at-spi2-core' 'cairo' 'dbus' 'expat' 'fontconfig' 'glib2' 'libcups'
         'libstdc++' 'libx11' 'libxcb' 'libxcomposite' 'libxdamage' 'libxext' 'libxfixes'
         'libxkbcommon' 'libxrandr' 'mesa' 'nspr' 'nss' 'pango' 'systemd-libs')
optdepends=('yt-dlp: gestreamte Videos herunterladen'
            'ffmpeg: Video und Ton heruntergeladener Streams zusammenfuehren'
            '1password-cli: Anmeldungen mit 1Password ausfuellen'
            'git: Erweiterungen aus Git installieren')
options=('!strip' '!debug' '!lto')

package() {
  install -Dm755 '$ROOT/target/release/atoll' "\$pkgdir$INSTALL_DIR/atoll"
  for file in ${CEF_RUNTIME[*]}; do
    install -Dm644 "$cef/\$file" "\$pkgdir$INSTALL_DIR/\$file"
  done
  chmod 755 "\$pkgdir$INSTALL_DIR"/*.so* "\$pkgdir$INSTALL_DIR/chrome-sandbox"
  cp -r '$cef/locales' "\$pkgdir$INSTALL_DIR/locales"
  install -Dm755 '$PACKAGE_DIR/atoll' "\$pkgdir/usr/bin/atoll"
  install -Dm644 '$PACKAGE_DIR/$APP_ID.desktop' "\$pkgdir/usr/share/applications/$APP_ID.desktop"
  for size in ${ICON_SIZES[*]}; do
    install -Dm644 "$PACKAGE_DIR/icons/\$size.png" \\
      "\$pkgdir/usr/share/icons/hicolor/\${size}x\${size}/apps/$APP_ID.png"
  done
  install -Dm644 '$cef/CREDITS.html' "\$pkgdir/usr/share/licenses/$PKGNAME/CEF-CREDITS.html"
}
EOF
}

build_package() {
  require cargo rust
  require makepkg pacman
  require fakeroot base-devel
  require magick imagemagick

  local version cef
  version="$(package_version)"
  cef="$(cef_directory)"

  log "Baue Atoll $version (Release, das dauert einige Minuten)."
  cargo build --release --locked -p atoll

  log "Rendere die Icons."
  mkdir -p "$PACKAGE_DIR"
  render_icons

  log "Schnuere das pacman-Paket."
  write_package_files "$version" "$cef"
  rm -f "$PACKAGE_DIR"/"$PKGNAME"-*.pkg.tar
  (
    cd "$PACKAGE_DIR"
    PKGDEST="$PACKAGE_DIR" SRCDEST="$PACKAGE_DIR/src-cache" BUILDDIR="$PACKAGE_DIR/build" \
      PKGEXT='.pkg.tar' makepkg --force --nodeps --noconfirm --cleanbuild
  )
}

latest_package() {
  local found
  found="$(ls -t "$PACKAGE_DIR"/"$PKGNAME"-*.pkg.tar 2>/dev/null | head -1 || true)"
  [[ -n "$found" ]] || die "Kein gebautes Paket unter $PACKAGE_DIR (ohne --no-build neu bauen)."
  printf '%s' "$found"
}

SKIP_BUILD=0
PACKAGE_ONLY=0
LAUNCH=0
DO_UNINSTALL=0

for arg in "$@"; do
  case "$arg" in
    --no-build) SKIP_BUILD=1 ;;
    --package) PACKAGE_ONLY=1 ;;
    --run) LAUNCH=1 ;;
    --uninstall) DO_UNINSTALL=1 ;;
    -h|--help) usage; exit 0 ;;
    *) die "Unbekannte Option: $arg (--help zeigt die Optionen)" ;;
  esac
done

[[ $EUID -ne 0 ]] || die "Bitte nicht als root starten, sudo wird bei Bedarf selbst gefragt."
require pacman

if [[ $DO_UNINSTALL -eq 1 ]]; then
  require sudo
  uninstall
  exit 0
fi

if [[ $SKIP_BUILD -eq 0 ]]; then
  build_package
else
  log "Ueberspringe den Build."
fi

PKGFILE="$(latest_package)"
log "Paket: ${PKGFILE#"$ROOT/"}"

if [[ $PACKAGE_ONLY -eq 1 ]]; then
  log "Fertig, installiert wird nichts. Spaeter mit: ./install.sh --no-build"
  exit 0
fi

require sudo
stop_running

if pacman -Qi "$PKGNAME" >/dev/null 2>&1; then
  log "Ueberschreibe die vorhandene Installation."
else
  log "Installiere Atoll."
fi

sudo pacman -U --noconfirm "$PKGFILE"

log "Fertig. Atoll ist im Anwendungsmenue und als 'atoll' im Terminal verfuegbar."

if [[ $LAUNCH -eq 1 ]]; then
  log "Starte Atoll."
  setsid atoll >/dev/null 2>&1 < /dev/null &
fi
