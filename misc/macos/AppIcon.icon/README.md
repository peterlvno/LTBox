# LTBox macOS Liquid Glass app icon

macOS-only Liquid Glass app icon source. **Not used on Windows or Linux** —
those keep `crates/ltbox-gui/assets/icon_source.svg`. `misc/macos/make-app.sh`
(step 4) renders this `.icon` into `AppIcon.icns` via Icon Composer's `ictool`
when it is available, and falls back to the cross-platform SVG (printing a
notice) otherwise.

This `.icon` was authored and validated with `ictool` from
Icon Composer 2 (also bundled in Xcode 26). It compiles cleanly and renders the
macOS Default / Dark appearances.

## Design

- **Background:** top-level `fill` — an `automatic-gradient` in the brand
  indigo→violet. The system adds the squircle mask, depth, and glass lighting.
- **Foreground (`Assets/hammer.svg`):** a **filled** rendering derived from the
  Lucide `hammer` glyph used on Windows/Linux — same hammer geometry, mirrored
  and centred, but the paths are closed and filled (one small sub-path dropped)
  rather than stroked, in **frosted white** (`automatic-gradient` near-white).
  `specular` is on for
  the glass sheen; a neutral shadow lifts it off the background. ictool honours
  filled paths, not strokes — hence the filled variant rather than the stroked
  Lucide line art.

Follows Apple's Liquid Glass icon guidance: simple, bold, solid filled shape;
let the system supply reflection / shadow / blur / highlights.

## `icon.json` schema notes (verified against ictool)

- A layer/canvas `fill` must be one of `solid`, `linear-gradient`,
  `automatic-gradient`, or `orientation`. Colors are
  `extended-srgb:R,G,B,A`.
- Keep the **background as the top-level `fill`**, not as its own glass group —
  a glass group behind a glass foreground washes the mark out to near-invisible.
- A foreground layer renders as clear glass unless given a `solid` /
  `automatic-gradient` body `fill`; strokes in the layer SVG are ignored, so
  the art must be filled paths.

## Build wiring

### Static `.icns` (works on every macOS — already wired)

`make-app.sh` step 4 finds `ictool` (standalone `Icon Composer.app`, Xcode 26
Developer tools, or `xcrun -f ictool`; override with `XCODE_APP`) and runs:

```bash
ictool AppIcon.icon --export-image --output-file icon_1024.png \
  --platform macOS --rendition Default --width 1024 --height 1024 --scale 1
```

then sizes it into an `.iconset` and `iconutil -c icns` → `AppIcon.icns`
(referenced by `CFBundleIconFile` in `Info.plist`). No `ictool` → it falls back
to the SVG and the macOS build looks exactly as it did before.

### Dynamic glass icon (optional, macOS 26 — follow-up)

The live light/dark/clear/tinted icon needs the compiled asset catalog, not a
static `.icns`. On an Xcode 26 host:

```bash
actool AppIcon.icon --compile "$APP/Contents/Resources" \
  --app-icon AppIcon --output-partial-info-plist /tmp/icon-plist \
  --platform macosx --minimum-deployment-target 26.0 \
  --output-format human-readable-text
```

then add `CFBundleIconName = AppIcon` to `Info.plist`. Verify the `actool`
flags against the installed Xcode 26 (they are not exercised by `make-app.sh`).
CI also needs a `macos-26` runner with Xcode 26 selected before either path
produces the glass icon (the current `macos-14` runner hits the SVG fallback).

## Re-editing

Open `AppIcon.icon` in Icon Composer to tweak the design; it owns `icon.json`
and `Assets/`. To preview a rendition from the CLI:

```bash
ictool AppIcon.icon --export-image --output-file /tmp/preview.png \
  --platform macOS --rendition Dark --width 1024 --height 1024 --scale 1
```
