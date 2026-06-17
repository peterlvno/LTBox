#!/usr/bin/env bash
#
# Assemble a universal (Apple Silicon + Intel) LTBox.app and package it as a
# .tar.gz for GitHub Releases (no App Store, no .dmg).
#
#   misc/macos/make-app.sh [OUTPUT_DIR]   # default: dist/macos
#
# Env:
#   SKIP_BUILD=1            reuse existing per-arch release binaries
#   MACOS_SIGN_IDENTITY=…   Developer ID Application identity → hardened-runtime
#                          sign + notarization-ready. Unset → ad-hoc sign (`-`).
#                          Ad-hoc is enough for non-App-Store distribution: it
#                          lets the binary launch, and a downloader just clears
#                          the Gatekeeper quarantine once (right-click → Open,
#                          or `xattr -dr com.apple.quarantine LTBox.app`).
#                          Developer-ID notarization only removes that one-time
#                          prompt and is optional (plan S4).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
cd "$REPO"

# Universal = both Mac architectures lipo'd into one binary.
TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
BIN_NAME="ltbox"
APP_NAME="LTBox"
OUT_DIR="${1:-$REPO/dist/macos}"
APP="$OUT_DIR/$APP_NAME.app"
TARBALL="$OUT_DIR/$APP_NAME-macos-universal.tar.gz"
ICON_SVG="$REPO/crates/ltbox-gui/assets/icon_source.svg"

# Workspace version → CFBundleShortVersionString.
VERSION="$(sed -n -E 's/^version = "([^"]+)".*/\1/p' "$REPO/Cargo.toml" | head -1)"
[ -n "$VERSION" ] || { echo "could not read version from Cargo.toml" >&2; exit 1; }

# 1. Build each arch with C deps statically linked so the bundle is
#    self-contained:
#      - LIBUSB_STATIC  → libusb1-sys vendors libusb.
#      - LZMA_API_STATIC → lzma-sys (via xz2 → noto-fonts-dl) compiles the
#        bundled liblzma from source. Without it, lzma-sys pkg-config's a
#        dynamic liblzma, which on a GitHub runner resolves to Homebrew's
#        /opt/homebrew/opt/xz/lib/liblzma.5.dylib — exactly the non-system
#        dylib the otool guard (step 5) rejects.
slices=()
for t in "${TARGETS[@]}"; do
    if [ "${SKIP_BUILD:-0}" != "1" ]; then
        rustup target add "$t" >/dev/null 2>&1 || true
        LIBUSB_STATIC=1 LZMA_API_STATIC=1 cargo build --release --locked --target "$t" -p ltbox-gui
    fi
    slice="$REPO/target/$t/release/$BIN_NAME"
    [ -x "$slice" ] || { echo "missing slice: $slice (run without SKIP_BUILD?)" >&2; exit 1; }
    slices+=("$slice")
done

# 2. Bundle skeleton + one universal binary.
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
lipo -create "${slices[@]}" -output "$APP/Contents/MacOS/$BIN_NAME"
printf 'APPL????' > "$APP/Contents/PkgInfo"

# 3. Info.plist (substitute the version).
sed "s/__SHORT_VERSION__/$VERSION/g" "$HERE/Info.plist" > "$APP/Contents/Info.plist"

# 4. AppIcon.icns.
#    macOS-only Liquid Glass: when Icon Composer's `ictool` is available, render
#    the .icns from the Icon Composer source (misc/macos/AppIcon.icon) so the
#    macOS build gets the frosted-glass hammer mark. `ictool` ships with the
#    standalone Icon Composer app and inside Xcode 26 (Developer tools) — not
#    the bare Command Line Tools — so a host without it automatically falls
#    back to rasterising the cross-platform app SVG with built-in tools
#    (qlmanage → sips → iconutil, no Homebrew). The fallback is byte-for-byte
#    the pre-Liquid-Glass path, so Windows/Linux art (icon_source.svg) is never
#    affected. Point at a specific Xcode with XCODE_APP=/path/to/Xcode.app.
#    A dynamic light/dark/clear/tinted icon (the full Liquid Glass experience
#    on macOS 26) additionally needs `actool` to compile AppIcon.icon into an
#    Assets.car + CFBundleIconName in Info.plist; that is documented in
#    AppIcon.icon/README.md as a follow-up for an Xcode-26 build host.
ICON_DIR="$HERE/AppIcon.icon"
iconset="$(mktemp -d)/AppIcon.iconset"
mkdir -p "$iconset"

ictool_bin() {
    # The Icon Composer `ictool` (the one that supports `--export-image`) lives
    # inside the Icon Composer app — standalone *and* embedded in Xcode. A
    # different, actool-style `ictool` sits at Xcode's Contents/Developer/usr/bin
    # and rejects `--export-image`, and `xcrun -f ictool` resolves to that one.
    # So prefer the Icon Composer.app executables and validate every candidate
    # against its own --help before accepting it.
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

if [ -d "$ICON_DIR" ] && ICT="$(ictool_bin)"; then
    echo "Icon: rendering Liquid Glass AppIcon.icns from $(basename "$ICON_DIR") via ictool"
    itmp="$(mktemp -d)"
    # Render the macOS Default appearance at full size; ictool bakes the icon
    # grid + bleed, so no extra padding is needed.
    "$ICT" "$ICON_DIR" --export-image --output-file "$itmp/icon_1024.png" \
        --platform macOS --rendition Default --width 1024 --height 1024 --scale 1
    [ -f "$itmp/icon_1024.png" ] || { echo "ictool produced no PNG (check the AppIcon.icon source)" >&2; exit 1; }
    src="$itmp/icon_1024.png"
else
    [ -d "$ICON_DIR" ] && echo "Icon: $(basename "$ICON_DIR") present but ictool not found — using the SVG fallback (install Icon Composer / Xcode 26, or set XCODE_APP, to build the glass icon)."
    qdir="$(mktemp -d)"
    qlmanage -t -s 1024 -o "$qdir" "$ICON_SVG" >/dev/null 2>&1
    src="$qdir/$(basename "$ICON_SVG").png"
    [ -f "$src" ] || { echo "icon rasterize failed (qlmanage produced no PNG)" >&2; exit 1; }
fi

gen() { sips -z "$2" "$2" "$src" --out "$iconset/$1" >/dev/null; }
gen icon_16x16.png 16;    gen icon_16x16@2x.png 32
gen icon_32x32.png 32;    gen icon_32x32@2x.png 64
gen icon_128x128.png 128; gen icon_128x128@2x.png 256
gen icon_256x256.png 256; gen icon_256x256@2x.png 512
gen icon_512x512.png 512; gen icon_512x512@2x.png 1024
iconutil -c icns "$iconset" -o "$APP/Contents/Resources/AppIcon.icns"

# 5. Guard against a non-self-contained bundle: no Homebrew/@rpath/libusb dylib.
if otool -L "$APP/Contents/MacOS/$BIN_NAME" \
    | tail -n +2 \
    | grep -Ei 'libusb|@rpath|/opt/homebrew|/usr/local/'; then
    echo "ERROR: bundle links a non-system dylib (above). Force static libusb." >&2
    exit 1
fi

# 6. Sign. Developer ID + hardened runtime when an identity is provided (S4),
#    else ad-hoc — arm64 requires at least an ad-hoc signature to run.
ENTITLEMENTS="$HERE/LTBox.entitlements"
if [ -n "${MACOS_SIGN_IDENTITY:-}" ]; then
    codesign --force --timestamp --options runtime \
        --entitlements "$ENTITLEMENTS" --sign "$MACOS_SIGN_IDENTITY" "$APP"
else
    codesign --force --entitlements "$ENTITLEMENTS" --sign - "$APP"
fi
codesign --verify --strict --verbose=2 "$APP"

# 7. Package as .tar.gz for the Release (like the Linux artifacts). The ad-hoc
#    signature lives inside the bundle, so it survives tar; COPYFILE_DISABLE
#    keeps AppleDouble `._*` metadata files out of the archive.
COPYFILE_DISABLE=1 tar -C "$OUT_DIR" -czf "$TARBALL" "$APP_NAME.app"

echo "Built $APP [$(lipo -archs "$APP/Contents/MacOS/$BIN_NAME")]"
echo "Packaged $TARBALL  (version $VERSION)"
