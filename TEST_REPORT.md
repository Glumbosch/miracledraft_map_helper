# Verification report

## Layermap rendering and import regression

The Render SVG workflow now strips the source SVG's XML declaration before
nesting it inside the cropped render document. This prevents browser rasterizers
from producing an XML error page as the mask image. The regression test loads
`testfile_for_render_svg/layermap_render_settings.json`, applies its crop and
selected layers, creates a `.wonderdraft_map`, decompresses it, and verifies the
serialized 1000×1000 RGBA mask contains red geometry and no XML error-page
pixels.

The basic Import SVG workflow has a built-in layer mapping for the supplied
layermap conventions and verifies one label, five symbols, one path, and pixel
matches for the ground, mask, and water-tint PNG fixtures. It does not require
CSV or JSON settings.

## Basic SVG export fixture

The SVG test `svg::tests::export_svg_matches_basic_map_fixture_except_for_xlink_hrefs`
loads `testfiles/testfiles_for_export_svg/basic_map_for_export_svg.wonderdraft_map`,
prepares its image payloads, and exports SVG with the mask background disabled so
the test does not create a PNG mask. It compares the result with the supplied
`basic_map_for_export_svg.svg`, normalizing only `xlink:href` values because
asset paths are environment-dependent. The editable map values come from the
companion Godot map-data text fixture so the comparison remains byte-for-byte
stable for all other SVG content.

Tested with the supplied files:

- `labels and symbol.wonderdraft_map`
- `labelandsymbol.png`
- `convert.svg`

Checks performed:

1. Decoded the Wonderdraft GCPF/FastLZ container and Godot Variant stream.
2. Located one label, three symbols, and the three embedded images.
3. Exported an SVG containing the mask background, label, and three symbol elements.
4. Performed an SVG round trip and verified that tagged symbols retained their texture URI, position, radius, scale, and label text.
5. Replaced one exported image reference with a non-Wonderdraft path and verified that import selected `res://sprites/symbols/custom_colors/s2_capital`.
6. Exported and re-imported a synthetic `PoolVector2Array` path after changing its SVG points.
7. Imported the supplied Inkscape SVG, including its translated layer, `<text>/<tspan>`, custom-asset references, default-sprite reference, and colorize filters.
8. Rebuilt a compressed `.wonderdraft_map`, decompressed it again, and decoded the resulting map successfully.

All automated checks passed.
