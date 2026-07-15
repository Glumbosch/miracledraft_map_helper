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

The background mask is embedded as a PNG data URI. Symbol images are referenced as file URIs so that an SVG editor can display and replace them while the importer can map them back to Wonderdraft asset paths.

The importer understands SVG view boxes, common physical units, nested transforms, Inkscape layer translations, text/tspan content, CSS-style attributes, and common path commands. Cubic and quadratic curves are flattened to point sequences when an arbitrary SVG path is imported.
