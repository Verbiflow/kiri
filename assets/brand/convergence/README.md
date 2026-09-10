# Kiri / Convergence

Alternate identity, concept 04. Preserved as a complete export pack.

## Files

- `mark/`: transparent silver, white, and black marks. Outlined SVG and vector PDF; PNG up to 2048px.
- `logo/`: horizontal and stacked lockups in silver, white, and black. SVG, PDF, and transparent PNG at 256, 512, 1024, and 2048px widths. Standalone wordmark SVGs included.
- `desktop/png/`: 16, 20, 24, 32, 40, 48, 64, 96, 128, 160, 192, 256, 512, 1024, and 2048px.
- `desktop/kiri.icns`: macOS icon container with 16/32/128/256/512-point representations at 1x and 2x.
- `desktop/kiri.iconset/`: editable macOS icon bundle.
- `desktop/kiri.ico`: Windows container with 16, 24, 32, 48, 64, 128, and 256px frames.
- `desktop/app-icon.svg`: desktop tile artwork.
- `desktop/app-icon-flat.svg`: entirely vector tile with a plain background.
- `web/`: responsive SVG favicon, multi-frame ICO, PNG favicons, 180px Apple touch icon, 192/512px PWA icons, separate maskable icons, pinned-tab SVG, manifest, and HTML integration snippet.
- `source/`: generated stencils, generation prompts.

## Use

Use silver or white transparent logos on dark backgrounds and black logos on light backgrounds. Preserve the artwork's transparent padding. Prefer SVG or PDF for large or print use.

The 16/20/24/32/40px desktop PNGs and 16/32px favicons use ordered pixel coverage for sharper detail. They intentionally differ from a smooth reduction of the large artwork. The responsive SVG favicon switches to coarser vector pixels at small viewport sizes.

The 2048px mark/logo PNGs are rasterized from the traced vector paths. All SVGs and PDFs contain vector artwork.

Copy the entire web folder together, then adjust the paths in `head.html` and the manifest's `id`/`start_url` to match the actual website. These are integration assets; no desktop app or website has been installed or deployed.

## Rebuild and verify

From the Kiri repository root on macOS:

```sh
swift scripts/export_brand.swift
swift scripts/verify_brand.swift
```

The exporter uses built-in macOS frameworks and iconutil; no third-party packages. Verification checks decoded PNG sizes and alpha, XML, PDF pages, ICO frame payloads, an ICNS unpack round-trip, manifest references, and the maskable safe area. `assets/brand/verification.json` records file hashes.

The underlying stencils and background were generated with the built-in image generation tool. The exporter traces the stencils into filled paths and adds custom outlined lowercase lettering, so production wordmarks have no external font dependency.

