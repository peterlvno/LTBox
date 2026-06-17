# LTBox macOS Liquid Glass app icon

macOS-only Liquid Glass app icon source. **Not used on Windows or Linux** —
those keep `crates/ltbox-gui/assets/icon_source.svg`. On macOS,
`misc/macos/make-app.sh` (step 4) compiles this `.icon` into a dynamic
`Assets.car` with `actool` (macOS 26 light/dark/clear/tinted) and ships a
full-res `AppIcon.icns` for macOS < 26 — see [Build wiring](#build-wiring).

This `.icon` was authored with Icon Composer and validated end-to-end:
`actool` compiles it to an `Assets.car` whose `AppIcon` carries the Aqua /
DarkAqua / Tintable appearances, and `ictool` renders the full-res legacy icns.

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

`make-app.sh` step 4 builds two things, independently:

### Dynamic Liquid Glass icon (macOS 26) — `Assets.car` via `actool`

This is what makes the icon respond to the light / dark / clear / tinted
appearances. `actool` ships in **every Xcode 26** (it is *not* Icon Composer —
no `ictool` / Apple-Developer download needed), so the `macos-26` CI runner has
it. `make-app.sh` runs roughly:

```bash
actool AppIcon.icon --compile "$APP/Contents/Resources" \
  --app-icon AppIcon --include-all-app-icons \
  --output-partial-info-plist /tmp/partial.plist \
  --enable-on-demand-resources NO --development-region en \
  --target-device mac --minimum-deployment-target 26.0 \
  --platform macosx --output-format human-readable-text
```

then drops `Assets.car` in `Contents/Resources/` and adds `CFBundleIconName =
AppIcon` to `Info.plist`. macOS 26 reads that and renders the dynamic icon.
`actool` is found via `XCODE_APP`, `/Applications/Xcode.app`, or `xcrun`. If
`actool` is absent (no Xcode) — or it fails (there is a reported `.icon`
actool crash on some Xcode 26.5 builds; pin a good Xcode via `XCODE_APP`) —
the step degrades gracefully to the static `.icns` only.

### Legacy `.icns` (macOS < 26) — `CFBundleIconFile`

A full-resolution static glass icon for systems that don't read `Assets.car`.
Source order: the committed **`../AppIcon.icns`** (regenerate with
`render-icon.sh`); else rasterise `crates/ltbox-gui/assets/icon_source.svg`
(the pre-Liquid-Glass path — Windows/Linux art unaffected). `actool` can emit
an `.icns` too, but only up to 256 px, so the committed full-res icns is
preferred for crisp Dock/Finder rendering on older macOS.

**After editing this `.icon`, regenerate the committed legacy icns and commit it:**

```bash
misc/macos/render-icon.sh        # writes misc/macos/AppIcon.icns (needs Icon Composer)
```

`render-icon.sh` uses Icon Composer's `ictool` (full-res render); only this
maintainer step needs Icon Composer — the app build (`make-app.sh`) needs just
`actool`.

## Re-editing

Open `AppIcon.icon` in Icon Composer to tweak the design; it owns `icon.json`
and `Assets/`. To preview a rendition from the CLI:

```bash
ictool AppIcon.icon --export-image --output-file /tmp/preview.png \
  --platform macOS --rendition Dark --width 1024 --height 1024 --scale 1
```
