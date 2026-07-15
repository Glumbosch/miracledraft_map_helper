# Verification report

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
