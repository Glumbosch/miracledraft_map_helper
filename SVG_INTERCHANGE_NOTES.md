# SVG interchange implementation notes

The SVG root uses a `wd` namespace:

```xml
xmlns:wd="urn:wonderdraft-map-editor"
```

Important attributes include:

- `wd:kind="background|label|symbol|path|territory"`
- `wd:record="..."` — URL-safe Base64 containing the original Godot text record.
- `wd:texture="user://assets/..."`, `res://sprites/...`, or `res://packs/...`
- `wd:export-width` and `wd:export-height` — rendered dimensions used to infer resizing on import.
- `wd:points-slot` and `wd:points-type` — location/type of a path's point collection.

The background mask can be embedded as a PNG data URI. Symbol images can be
referenced as file URIs, or embedded once as PNG data definitions and placed as
`<use>` clones. The importer maps either representation back to Wonderdraft
asset paths and records.

The importer understands SVG view boxes, common physical units, nested transforms, Inkscape layer translations, text/tspan content, CSS-style attributes, and common path commands. Cubic and quadratic curves are flattened to point sequences when an arbitrary SVG path is imported.
## Custom-color symbols

When a symbol record enables `custom_color_mode` and contains three valid
`custom_colors`, SVG export creates a deduplicated `<feColorMatrix>` filter.
Matrix columns one through three map the sprite's red, green, and blue channels
to the corresponding Wonderdraft colors. A following `<feComposite>` combines
the mapped color alpha with the original sprite transparency. Both external
symbol images and embedded `<use>` clones reference the filter.

## Symbol rotation, mirroring, and outlines

Wonderdraft symbol rotations are stored in radians. Positive values rotate
clockwise and negative values counter-clockwise because both Wonderdraft and
SVG use a downward-positive Y axis. `mirror: true` performs a vertical flip
before the rotation is applied.

When `outline_width` is positive and `outline_color` is a valid `Color`, export
adds a deduplicated outline filter. It dilates `SourceAlpha`, removes the
original source shape, floods the remaining outline with the configured color
and alpha, and merges the original symbol on top. The filter region is expanded
so the outline is not clipped.

## Paths and territories

Roads and territories export as SVG `<path>` elements with `wd:kind="path"` or
`wd:kind="territory"`. The original record remains in `wd:record`, while path
endpoint edits are converted back to the record's original string, array, or
pool-vector representation during import. The importer also continues to
accept older `points` attributes. Group IDs are organizational only: the
importer scans the full document for `wd:kind`, applies ancestor transforms,
and does not require an element to remain in its exported group. Presentation
styles must be placed on the tagged element rather than only on an ancestor.

For roads, imported `stroke`, `stroke-opacity`, and `stroke-width` update the
record's color and width. For territories, imported `fill`, `fill-opacity`, and
`stroke-width` update color, opacity, and border width. CSS declarations in the
element's `style` attribute take precedence over same-named presentation
attributes, matching Inkscape's output.

The territory color is used for the fill and ordinary border. Fill opacity
comes from the record's `opacity`; borders are drawn at full opacity.
`border_dash` adds an SVG dash pattern. `border_dark_dot` uses a black,
round-capped dotted line and converts Wonderdraft width to SVG width with a
factor of `0.42`. `border_gradient` emits a separate solid border at twice the
configured width and applies a reusable `feGaussianBlur` with
`stdDeviation="10"`, keeping the fill sharp.

## Label fonts, outlines, and glow

`wonderdraft_font_names.txt` maps Wonderdraft's filename-derived font labels to
the family, style, and weight stored inside the actual font. The setup wizard
extracts names from core and custom-pack fonts and appends missing mappings;
existing lines remain user-editable and are never replaced automatically.

Label strokes use `paint-order="markers stroke fill"` so the fill is painted
over the inner half of the outline. Positive `glow_size` values create a
deduplicated flood, Gaussian-blur, zero-offset, and composite filter using
`glow_color`; `glow_size` is the SVG `stdDeviation`. A zero glow size produces
no filter.
