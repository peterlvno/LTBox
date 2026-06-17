#!/usr/bin/env bash
#
# Regenerate misc/macos/AppIcon.icns from misc/macos/AppIcon.icon using Icon
# Composer's `ictool`.
#
# Why a committed .icns: the `.icon` is the editable Liquid Glass source, but
# rendering it needs Icon Composer (standalone app or bundled in Xcode 26),
# which not every build host has. make-app.sh prefers a live ictool render, but
# ships the committed AppIcon.icns as a fallback so a host without Icon Composer
# never regresses to the old art. Run this on a Mac that has Icon Composer after
# editing the .icon, then commit the updated AppIcon.icns.
#
#   misc/macos/render-icon.sh            # writes misc/macos/AppIcon.icns
#   XCODE_APP=/path/to/Xcode.app misc/macos/render-icon.sh
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ICON_DIR="$HERE/AppIcon.icon"
OUT="$HERE/AppIcon.icns"
[ -d "$ICON_DIR" ] || { echo "missing $ICON_DIR" >&2; exit 1; }

# The Icon Composer `ictool` (which supports `--export-image`) lives inside the
# Icon Composer app — standalone and embedded in Xcode. A different actool-style
# `ictool` sits at Xcode's Contents/Developer/usr/bin and rejects
# `--export-image`, and `xcrun -f ictool` resolves to that one. So prefer the
# Icon Composer.app executables and validate each candidate via its --help.
ictool_bin() {
    local c h
    for c in \
        ${XCODE_APP:+"$XCODE_APP/Contents/Applications/Icon Composer.app/Contents/Executables/ictool"} \
        "/Applications/Icon Composer.app/Contents/Executables/ictool" \
        "/Applications/Xcode.app/Contents/Applications/Icon Composer.app/Contents/Executables/ictool" \
        ${XCODE_APP:+"$XCODE_APP/Contents/Developer/usr/bin/ictool"} \
        "/Applications/Xcode.app/Contents/Developer/usr/bin/ictool" \
        "$(xcrun -f ictool 2>/dev/null || true)"; do
        [ -n "$c" ] && [ -x "$c" ] || continue
        h="$("$c" --help 2>&1 || true)"
        case "$h" in *--export-image*) printf '%s\n' "$c"; return 0 ;; esac
    done
    return 1
}

ICT="$(ictool_bin)" || {
    echo "Icon Composer 'ictool' (with --export-image) not found." >&2
    echo "Install Icon Composer or Xcode 26, or set XCODE_APP=/path/to/Xcode.app." >&2
    exit 1
}
echo "Using ictool: $ICT"

tmp="$(mktemp -d)"
iconset="$tmp/AppIcon.iconset"
mkdir -p "$iconset"
# Render the macOS Default appearance at full size; ictool bakes the icon grid.
"$ICT" "$ICON_DIR" --export-image --output-file "$tmp/icon_1024.png" \
    --platform macOS --rendition Default --width 1024 --height 1024 --scale 1
[ -f "$tmp/icon_1024.png" ] || { echo "ictool produced no PNG" >&2; exit 1; }

gen() { sips -z "$2" "$2" "$tmp/icon_1024.png" --out "$iconset/$1" >/dev/null; }
gen icon_16x16.png 16;    gen icon_16x16@2x.png 32
gen icon_32x32.png 32;    gen icon_32x32@2x.png 64
gen icon_128x128.png 128; gen icon_128x128@2x.png 256
gen icon_256x256.png 256; gen icon_256x256@2x.png 512
gen icon_512x512.png 512; gen icon_512x512@2x.png 1024
iconutil -c icns "$iconset" -o "$OUT"
echo "Wrote $OUT"
