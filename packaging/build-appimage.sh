#!/usr/bin/env bash
# Builds rv-gtk-x86_64.AppImage from a built rv-gtk. Used by .github/workflows/release.yml, which
# downloads and SHA-256-checks the tools first; this script trusts the paths it is given.
#
#   RV_GTK        the rv-gtk binary (stripped), copied in as usr/bin/rv-gtk
#   OUT           the AppImage to write
#   LINUXDEPLOY   linuxdeploy-x86_64.AppImage
#   GTK_PLUGIN    linuxdeploy-plugin-gtk.sh
#   APPIMAGETOOL  appimagetool-x86_64.AppImage
#   RUNTIME       runtime-x86_64, the static type-2 runtime (it needs no libfuse2)
#
# GTK4 and what it loads (GSettings schemas, gdk-pixbuf loaders, GTK modules) are copied from the
# system it runs on, with the licence text of every Debian package whose files were bundled.
set -euo pipefail

for v in RV_GTK OUT LINUXDEPLOY GTK_PLUGIN APPIMAGETOOL RUNTIME; do
  [ -n "${!v:-}" ] || { echo "build-appimage: $v is not set" >&2; exit 2; }
  [ -f "${!v}" ] || { echo "build-appimage: $v=${!v} is not a file" >&2; exit 2; }
done

here=$(cd "$(dirname "$0")/.." && pwd)
data="$here/crates/rv-gtk/data"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
appdir="$work/AppDir"

# linuxdeploy and appimagetool are AppImages; the runner has no FUSE to mount them with.
export APPIMAGE_EXTRACT_AND_RUN=1
export ARCH=x86_64
export DEPLOY_GTK_VERSION=4
export NO_STRIP=1

# linuxdeploy finds `--plugin gtk` as linuxdeploy-plugin-gtk.sh next to itself or on PATH.
mkdir "$work/bin"
cp "$LINUXDEPLOY" "$work/bin/linuxdeploy"
cp "$GTK_PLUGIN" "$work/bin/linuxdeploy-plugin-gtk.sh"
chmod +x "$work/bin/linuxdeploy" "$work/bin/linuxdeploy-plugin-gtk.sh"
export PATH="$work/bin:$PATH"

cp "$RV_GTK" "$work/rv-gtk"
chmod +x "$work/rv-gtk"

linuxdeploy --appdir "$appdir" \
  --executable "$work/rv-gtk" \
  --desktop-file "$data/rs.rv.gtk.desktop" \
  --icon-file "$data/rs.rv.gtk.svg" \
  --icon-file "$data/rs.rv.gtk.png" \
  --plugin gtk

ls -l "$appdir" "$appdir/apprun-hooks"

# The plugin's hook forces GDK_BACKEND=x11 ("crash with Wayland backend on Wayland", a GTK3-era
# workaround). rv-gtk bundles its own GTK4 with libwayland, so it runs on the display it finds.
# If a newer plugin drops the line, this fails and the pin gets looked at.
hook="$appdir/apprun-hooks/linuxdeploy-plugin-gtk.sh"
grep -q '^export GDK_BACKEND=x11' "$hook" || { echo "build-appimage: the GDK_BACKEND line is not in $hook" >&2; exit 1; }
sed -i '/^export GDK_BACKEND=x11/d' "$hook"

# The icon theme GTK's own widgets (sort arrows, window buttons) draw from. The plugin copies none.
mkdir -p "$appdir/usr/share/icons"
cp -rL /usr/share/icons/Adwaita "$appdir/usr/share/icons/"

# The licence texts. Every bundled library comes from a Debian package; its copyright file names
# the licence, and the licences it points to (/usr/share/common-licenses) go in too.
doc="$appdir/usr/share/doc"
mkdir -p "$doc/rv-rs"
cp "$here/LICENSE" "$here/NOTICE" "$doc/rv-rs/"
command -v dpkg-query >/dev/null || { echo "build-appimage: dpkg-query not found, cannot name the licences" >&2; exit 1; }
: > "$work/packages"
while IFS= read -r -d '' lib; do
  rel=${lib#"$appdir"}
  # The bundled copy keeps the system path (/usr/lib/x86_64-linux-gnu/...) or sits in usr/lib.
  for cand in "$rel" "/usr/lib/x86_64-linux-gnu/$(basename "$lib")" "/lib/x86_64-linux-gnu/$(basename "$lib")"; do
    if p=$(dpkg-query -S "$cand" 2>/dev/null | head -n1 | cut -d: -f1); [ -n "$p" ]; then
      echo "$p" >> "$work/packages"; break
    fi
  done
done < <(find "$appdir/usr" \( -type f -o -type l \) -name '*.so*' -print0)
echo adwaita-icon-theme >> "$work/packages"
sort -u "$work/packages" -o "$work/packages"
[ -s "$work/packages" ] || { echo "build-appimage: no Debian package found for any bundled library" >&2; exit 1; }
while read -r pkg; do
  src="/usr/share/doc/$pkg/copyright"
  [ -f "$src" ] || { echo "build-appimage: $src is missing" >&2; exit 1; }
  mkdir -p "$doc/$pkg"
  cp "$src" "$doc/$pkg/copyright"
done < "$work/packages"
cp -rL /usr/share/common-licenses "$doc/common-licenses"
cp "$work/packages" "$doc/rv-rs/bundled-packages.txt"
echo "licence texts for $(wc -l < "$work/packages") packages:"; cat "$work/packages"

# The static type-2 runtime is given by file, so appimagetool downloads nothing.
appimagetool --no-appstream --runtime-file "$RUNTIME" "$appdir" "$OUT"
chmod +x "$OUT"
ls -l "$OUT"
