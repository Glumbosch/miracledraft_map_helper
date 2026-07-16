# SVG interchange implementation notes

The SVG root uses a `wd` namespace:

```xml
xmlns:wd="urn:wonderdraft-map-editor"
```

Important attributes include:

- `wd:kind="background|label|symbol|path"`
- `wd:record="..."` — URL-safe Base64 containing the original Godot text record.
- `wd:texture="user://assets/..."` or `res://sprites/...`
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
