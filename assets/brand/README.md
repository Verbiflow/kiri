# Kiri identity assets

Horizon, concept 05, is the preferred direction. It uses a silver pixel-fog mark on a dark gunmetal fog background. Convergence, concept 04, remains available as an alternate pack.

- [Horizon assets](horizon/README.md)
- [Convergence assets](convergence/README.md)
- [Comparison preview](preview.png)
- [Verification and SHA-256 hashes](verification.json)

Each pack includes transparent marks, horizontal and stacked logos, SVG/PDF/PNG exports, desktop ICO/ICNS/iconset files, and web favicons/PWA icons. The textured Horizon app SVG embeds a raster background beneath vector foreground paths; the flat variant and all mark/logo SVGs are entirely vector.

Run `sh scripts/package_brand.sh` from the repository root on macOS to rebuild, verify, and create both ZIPs. You can also run `swift scripts/export_brand.swift` and `swift scripts/verify_brand.swift` separately. The generated source artwork and prompts are retained in each pack's `source` folder.

Verification also compares the exported silhouette directly with the generated source using an independent pixel decoder. This protects against a shared tracing error across otherwise consistent output formats, including padded RGB pixel storage.

This directory contains design assets only. Application behavior and repository state are unaffected.
