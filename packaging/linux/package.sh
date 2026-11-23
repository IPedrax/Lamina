#!/usr/bin/env bash
# Build and package Lamina for Linux (<arch> is the host's, x86_64 or aarch64):
#
#   $DIST/lamina-<version>-linux-<arch>.AppImage  any distro with glibc >= the build host's
#   $DIST/lamina-<version>-linux-<arch>.tar.gz    a plain tree (bin/, share/)
#
# Usage: packaging/linux/package.sh [--skip-build] [--formats "appimage tar"]
#
# Needs cargo, and appimagetool for the AppImage (downloaded into $CARGO_TARGET_DIR if missing). Build on an old distro (CI:
# Ubuntu 22.04, glibc 2.35) so the binaries run on newer ones. desktop-file-validate checks the desktop entry when it is there.
# Lifted from BIMCraft's packaging/linux/package.sh (same author, MIT OR Apache-2.0), without its deb, rpm and cross builds.
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
HERE="$ROOT/packaging/linux"
APP_ID=io.github.ipedrax.lamina

SKIP_BUILD=0
FORMATS="appimage tar"
while [ $# -gt 0 ]; do
  case "$1" in
    --skip-build) SKIP_BUILD=1; shift ;;
    --formats) FORMATS="$2"; shift 2 ;;
    -h | --help) sed -n '2,12p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

ARCH="$(uname -m)"
case "$ARCH" in
  x86_64) ;;
  aarch64 | arm64) ARCH=aarch64 ;;
  *) echo "unsupported architecture $ARCH" >&2; exit 2 ;;
esac
BASENAME="lamina-$VERSION-linux-$ARCH"
echo "==> Lamina $VERSION for Linux $ARCH ($FORMATS)"

if [ "$SKIP_BUILD" = 0 ]; then
  (cd "$ROOT" && cargo build --release --locked -p lamina -p lamina-cli)
fi
BIN="$CARGO_TARGET_DIR/release"
WORK="$CARGO_TARGET_DIR/linux-package"
STAGE="$WORK/root"
rm -rf "$WORK"

# ---- a tree every format is made from ----------------------------------------------------------
install -Dm755 "$BIN/lamina" "$STAGE/usr/bin/lamina"
install -Dm755 "$BIN/lamina-cli" "$STAGE/usr/bin/lamina-cli"
strip "$STAGE/usr/bin/lamina" "$STAGE/usr/bin/lamina-cli"
install -Dm644 "$HERE/$APP_ID.desktop" "$STAGE/usr/share/applications/$APP_ID.desktop"
install -Dm644 "$HERE/$APP_ID.mime.xml" "$STAGE/usr/share/mime/packages/$APP_ID.xml"
install -Dm644 "$ROOT/assets/app-icon/lamina-256.png" "$STAGE/usr/share/icons/hicolor/256x256/apps/$APP_ID.png"
install -Dm644 "$ROOT/assets/app-icon/lamina-1024.png" "$STAGE/usr/share/icons/hicolor/1024x1024/apps/$APP_ID.png"
mkdir -p "$STAGE/usr/share/doc/lamina"
copy_docs "$STAGE/usr/share/doc/lamina"

if command -v desktop-file-validate >/dev/null; then
  desktop-file-validate "$STAGE/usr/share/applications/$APP_ID.desktop"
fi

has() { case " $FORMATS " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }

# ---- .tar.gz -----------------------------------------------------------------------------------
if has tar; then
  mkdir -p "$WORK/tar"
  cp -R "$STAGE/usr" "$WORK/tar/$BASENAME"
  tar -C "$WORK/tar" -czf "$DIST/$BASENAME.tar.gz" "$BASENAME"
  echo "wrote $DIST/$BASENAME.tar.gz"
fi

# ---- AppImage ----------------------------------------------------------------------------------
if has appimage; then
  APPDIR="$WORK/Lamina.AppDir"
  cp -R "$STAGE" "$APPDIR"
  ln -s usr/bin/lamina "$APPDIR/AppRun"
  cp "$HERE/$APP_ID.desktop" "$APPDIR/$APP_ID.desktop"
  cp "$ROOT/assets/app-icon/lamina-256.png" "$APPDIR/$APP_ID.png"
  ln -s "$APP_ID.png" "$APPDIR/.DirIcon"

  TOOL="${APPIMAGETOOL:-$(command -v appimagetool || true)}"
  if [ -z "$TOOL" ]; then
    TOOL="$CARGO_TARGET_DIR/appimagetool-$ARCH.AppImage"
    if [ ! -x "$TOOL" ]; then
      curl -fsSL -o "$TOOL" "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$ARCH.AppImage"
      chmod +x "$TOOL"
    fi
  fi
  TOOL="$(cd "$(dirname "$TOOL")" && pwd)/$(basename "$TOOL")"
  OUT="$DIST/$BASENAME.AppImage"
  # Extract-and-run works without FUSE (containers, CI); the AppImage it writes embeds the static runtime, so users don't need
  # libfuse2 either.
  (cd "$DIST" && ARCH="$ARCH" APPIMAGE_EXTRACT_AND_RUN=1 "$TOOL" --no-appstream "$APPDIR" "$OUT")
  echo "wrote $OUT"
fi

"$STAGE/usr/bin/lamina-cli" --version
"$STAGE/usr/bin/lamina" --version
echo "==> done"
