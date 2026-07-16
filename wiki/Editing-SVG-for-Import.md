# Editing SVG for Import

Wonderdraft Map Editor imports tagged SVG elements. The element's parent group
does **not** decide whether it is imported; the `wd:kind` attribute on the
element does.

## Which group must an element be in?

No particular group is required. The importer scans the entire SVG document
and recognizes tagged elements wherever they appear. Parent and element
`transform` attributes are applied to imported geometry.

The exporter creates these groups for organization:

- `wonderdraft-paths`
- `wonderdraft-territories`
- `wonderdraft-symbols`
- `wonderdraft-labels`

Keeping elements in their exported groups is recommended because it makes the
file easier to understand, but group IDs and element IDs are not used for
recognition. Moving a tagged element to another group does not stop it from
being imported.

Apply editable colors and widths directly to the tagged element. The importer
does not inherit `fill`, `stroke`, or `stroke-width` from a parent group's CSS
style. Inkscape normally writes the chosen style onto the selected element.

## Common required metadata

The SVG root must keep the Wonderdraft namespace declaration:

```xml
xmlns:wd="urn:wonderdraft-map-editor"
```

Every importable element needs `wd:kind`:

```xml
wd:kind="path"
wd:kind="territory"
wd:kind="symbol"
wd:kind="label"
```

Keep `wd:record` as well. It contains the URL-safe Base64 encoding of the
original Wonderdraft record. The importer can create a minimal record when it
is missing, but fields that SVG cannot express—such as path roughness, border
style, symbol type, or label glow—will be lost. For a reliable round trip,
treat both `wd:kind` and `wd:record` as required and do not edit the encoded
record by hand.

`id` and Inkscape-specific attributes are not required.

## Roads and paths

A road/path should remain a `<path>` with:

```xml
<path
  d="M 100,100 L 300,140"
  stroke="#ff0000"
  stroke-opacity="1"
  stroke-width="12.5"
  fill="none"
  wd:kind="path"
  wd:record="..." />
```

The importer reads:

- `d` for the edited points; the older `points` attribute is also accepted
- `transform`, including transforms inherited from parent groups
- `stroke` as the Wonderdraft path color
- `stroke-opacity` and the general `opacity` as color alpha
- `stroke-width` as the Wonderdraft path width

CSS properties inside the element's `style` attribute override matching XML
presentation attributes, as required for Inkscape output. For example, this is
imported as a red path even if an older `stroke="#2a432f"` is still present:

```xml
style="stroke:#ff0000;stroke-width:18"
```

Hex colors in `#RGB`, `#RGBA`, `#RRGGBB`, and `#RRGGBBAA` form are supported.

Keep `wd:record` to preserve path fields that are not visually editable in SVG,
including `style`, `roughness`, `straight`, `noise_seed`, and `z_index`.

## Territories

Edit the primary `<path wd:kind="territory">` element:

```xml
<path
  d="M 100,100 L 300,100 L 200,300 Z"
  fill="#ffff00"
  fill-opacity="0.25"
  stroke="#ffff00"
  stroke-width="10"
  wd:kind="territory"
  wd:style="res://textures/borders/border_dash"
  wd:record="..." />
```

The importer reads:

- `d` and `transform` for territory points
- `fill` as the Wonderdraft territory color
- `fill-opacity` and the general `opacity` as Wonderdraft territory opacity
- `stroke-width` as territory border width

For `border_dark_dot`, the importer converts the SVG's 0.42-scaled visible
stroke width back to the Wonderdraft width. Keep `wd:style` and `wd:record` so
the dashed, dotted, gradient, or ordinary border style survives the round trip.

Some exported territory styles include an additional decorative border path
with `wd:role="territory-border"`. Make color and width changes on the primary
path carrying `wd:kind="territory"`; an untagged decorative path is not itself
imported as a territory.

## Symbols

Exported symbols use `<image>`, `<use>`, or a fallback `<circle>`. Keep:

- `wd:kind="symbol"`
- `wd:record`
- `wd:texture`
- the element geometry (`x`, `y`, `width`, `height`, or circle geometry)
- `transform`
- the source/export size and offset metadata written by the exporter

The source/export dimensions, base radius, and offsets let the importer convert
visual resizing back into Wonderdraft scale and position. `href` or `xlink:href`
may be used to resolve a changed texture, but `wd:texture` is the reliable
Wonderdraft asset reference.

## Labels

Keep the exported `<text>` element with:

- `wd:kind="label"`
- `wd:record`
- `x`, `y`, `font-size`, and its text content
- `font-family`, `font-style`, and `font-weight`
- `transform` when the label is rotated or moved through a transform

The importer updates position, size, rotation, text, and mapped Wonderdraft
font. The encoded record preserves other label fields.

## Background mask

The exported mask image has `wd:kind="background"`, but SVG import does not
replace the embedded mask image. Use **Embedded images → mask → Replace PNG**
for that operation.

## Safe Inkscape workflow

1. Export the required layers from Wonderdraft Map Editor.
2. Open the SVG in Inkscape.
3. Select and edit the existing tagged element. Do not use **Flatten**,
   **Unlink Clone**, or an optimizer that removes namespaced attributes.
4. For a duplicate, duplicate the complete tagged element so its `wd:kind` and
   `wd:record` are retained.
5. Save as ordinary Inkscape SVG or plain SVG while preserving the `wd`
   namespace and attributes.
6. Import the SVG, review the imported item counts, and use **Save map as…**.
7. Open the new map in Wonderdraft and verify it visually.

If the import count is zero, first check that the element still has `wd:kind`
and that the SVG root still declares `xmlns:wd`.
