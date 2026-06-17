# LTBox macOS Liquid Glass app icon

macOS-only Liquid Glass app icon source. **Not used on Windows or Linux** —
those keep `crates/ltbox-gui/assets/icon_source.svg`. `misc/macos/make-app.sh`
(step 4) renders this `.icon` into `AppIcon.icns` via Icon Composer's `ictool`
when available, otherwise ships the committed `../AppIcon.icns`, and only
rasterises the cross-platform SVG if neither is present (the three tiers are
detailed under [Build wiring](#build-wiring)).

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

### Static `.icns` (already wired) — three tiers

`make-app.sh` step 4 resolves `AppIcon.icns` in order:

1. **`ictool` present** → render fresh from this `.icon`. `ictool` is found in
   the standalone `Icon Composer.app`, inside Xcode 26, or via `xcrun`
   (override with `XCODE_APP`), and validated to support `--export-image`:
   ```bash
   ictool AppIcon.icon --export-image --output-file icon_1024.png \
     --platform macOS --rendition Default --width 1024 --height 1024 --scale 1
   ```
   then `sips` into an `.iconset` and `iconutil -c icns`. CI builds on
   `macos-26` (Xcode 26), so this tier runs there.
2. **No `ictool`, committed `../AppIcon.icns` exists** → ship that pre-rendered
   glass icon. The committed `misc/macos/AppIcon.icns` is the safety net so a
   host without Icon Composer never regresses to the old art.
3. **Neither** → rasterise `crates/ltbox-gui/assets/icon_source.svg` (the
   pre-Liquid-Glass path; Windows/Linux art unaffected).

The result is referenced by `CFBundleIconFile` in `Info.plist`.

**After editing this `.icon`, regenerate the committed icns and commit it:**

```bash
misc/macos/render-icon.sh        # writes misc/macos/AppIcon.icns
```

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
Since CI now runs `build-macos` on `macos-26`, the toolchain for this is
available there when someone wants to wire it up.

## Re-editing

Open `AppIcon.icon` in Icon Composer to tweak the design; it owns `icon.json`
and `Assets/`. To preview a rendition from the CLI:

```bash
ictool AppIcon.icon --export-image --output-file /tmp/preview.png \
  --platform macOS --rendition Dark --width 1024 --height 1024 --scale 1
```
