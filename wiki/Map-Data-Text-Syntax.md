# Map-data text syntax

This guide explains the readable **map data text** shown by Miracledraft Map
Helper after it decodes a `.wonderdraft_map` file. The field shapes below are
illustrated by `tempfiles/mapdata_example_string.txt`, a version-15, 512 × 512
map. Other Wonderdraft versions and maps can contain additional fields. For the
outer binary GCPF container and Godot Variant encoding, see [Wonderdraft Map File Format](wonderdraft_map-fileformat).

> This is Godot-style text, not JSON. It is an editable representation of the
> decoded map data. Preserve the structure and unfamiliar fields when editing.

## Basic structure

The complete map is one Dictionary, delimited by `{` and `}`. A dictionary is
made of quoted key/value pairs separated by a colon. Arrays use `[` and `]`.
Values are separated by commas.

```gdscript
{
  "map_width": 1024,
  "map_height": 768,
  "symbols": [ { ... }, { ... } ],
  "labels": [ ... ],
  "theme": { ... }
}
```

Whitespace and line breaks are for readability. Quoted strings use normal
escaped-string rules. The text can additionally contain Godot value forms such
as `null`, `true`, `false`, `Vector2(...)`, `Color(...)`, `Rect2(...)`,
`Object(...)`, and packed arrays such as `PoolByteArray(...)`.

The helper represents large embedded binary values with file-like placeholders
such as `.mask.png`, `.ground.png`, `.water_tint.png`, and `.image.png`. Those
are references to the helper's extracted, disk-backed image data; they are not
paths that Wonderdraft resolves from an asset pack. Do not rename or replace a
placeholder in the text manually. Use the embedded-image panel so the matching
image object and pixels are restored on save.

## Reading paths into the map

Use a dot-separated path to describe a nested value. An array index is written
as a number:

```text
symbols.0.texture
boxes.0.properties.texture.properties.image
territories.territories.2.width
```

The first example is the `texture` field of the first symbol. The second is the
embedded image used by the first box. These paths identify data in the decoded
tree; they are not literal keys to type into the file.

## Common value forms

| Form | Example | Meaning |
| --- | --- | --- |
| String | `"City"` | Text, resource path, style name, or another named value. |
| Number | `48`, `0.35` | Integer or decimal number. |
| Boolean | `true` | On/off value. |
| Null | `null` | No value is set. |
| `Vector2` | `Vector2(66, 422.5)` | Two coordinates or a two-axis scale/offset. |
| `Color` | `Color(1, 0.678431, 0, 0.988235)` | Red, green, blue, alpha components, normally in the range 0–1. |
| Dictionary | `{ "key": value }` | Named fields grouped together. |
| Array | `[ value1, value2 ]` | Ordered list of records or values. |
| Object | `Object("NinePatchRect", { ... })` | Named Godot object class followed by its properties. |
| Packed array | `PoolColorArray(...)` | Compact binary-array type formatted as readable values. |

## Example map at a glance

The example's top-level dictionary contains the following groups. This is a
useful orientation map, not a required schema.

| Group | Example keys | What it controls |
| --- | --- | --- |
| Canvas and file state | `map_width`, `map_height`, `version`, `scale`, `sharpen_labels` | Map dimensions and general map state. The example has `version: 15`. |
| Embedded paint layers | `mask`, `ground`, `water_tint` | The helper's image placeholders for full-map raster data. |
| Placed content | `boxes`, `labels`, `paths`, `symbols`, `windroses` | Objects drawn on top of the map. |
| Territory content | `territories.territories` | Nested list of territory shapes. |
| Layer controls | `layers.enabled`, `layers.names`, `layers.visibility`, `layers.lock` | Named drawing layers and their state. |
| Resources | `included_packs`, `included_default_packs`, `load_default_symbols`, `load_default_not_symbols` | Asset-pack identifiers and loading choices. |
| Rendering and theme | `frame`, `grid`, `theme`, `water_*`, `trace` | Frame/grid options, appearance settings, and optional tracing. |

`layers.names`, `layers.visibility`, and `layers.lock` are parallel arrays in
the example: entry `i` in each array applies to the same named layer. Keep the
arrays aligned if you edit them.

## Symbols

`symbols` is an array. Each entry is a Dictionary describing one placed
Wonderdraft symbol:

```gdscript
"symbols": [
  {
    "custom_color_mode": null,
    "custom_colors": null,
    "mirror": false,
    "offset": Vector2(0, 0),
    "outline_color": Color(1, 1, 1, 1),
    "outline_width": 0,
    "position": Vector2(66, 422.5),
    "radius": null,
    "rotation": 0.0,
    "sample": null,
    "scale": Vector2(0.35, 0.35),
    "texture": "user://assets/volcano_pack/sprites/mountains/Imported Symbols/volcano",
    "type": "mountain",
    "z_index": 0
  }
]
```

### Placement and appearance

| Field | Meaning |
| --- | --- |
| `position` | Symbol placement on the map in map-pixel coordinates. |
| `scale` | Horizontal and vertical sprite scale. |
| `rotation` | Rotation in radians. Positive values rotate clockwise in Wonderdraft's map/SVG coordinate system. |
| `mirror` | When `true`, the sprite is flipped vertically before rotation. |
| `z_index` | Draw-order value. |
| `outline_color`, `outline_width` | Optional outline appearance. |
| `custom_color_mode`, `custom_colors` | Optional channel-based custom color settings. Preserve their relationship rather than setting only one field. |

### Texture and type

`texture` is a resource URI, not necessarily a literal PNG filename. It may
omit the installed file extension. Typical paths are:

```gdscript
"res://sprites/symbols/bridges_paintable/bridge_01_flat"
"res://packs/example_pack/sprites/mountains/volcano"
"user://assets/volcano_pack/sprites/mountains/Imported Symbols/volcano"
```

- `res://sprites/...` normally identifies a core Wonderdraft sprite.
- `res://packs/...` identifies a resource in a bundled/default pack.
- `user://assets/...` identifies an installed custom asset pack. Its location
  is determined by Wonderdraft's asset-folder settings on the target computer.

`type` is the symbol category, such as `mountain`; it is **not** always the
literal value `"symbol"`. Valid categories depend on the supplied asset and
the Wonderdraft version.

### Paintable-symbol fields

Some symbols support painting from the map's ground or water colors. They may
include the following optional fields:

| Field | Meaning |
| --- | --- |
| `radius` | Radius of the paintable/sample area. `null` means no explicit radius is stored. |
| `offset` | `Vector2(x, y)` offset of that area relative to the texture centre. |
| `sample` | A sampled `Color(r, g, b, a)` used by the paintable draw mode. |

Whether these values have an effect is determined by the sprite's metadata and
draw mode. Do not add them to an unrelated sprite merely because another symbol
contains them.

### Custom-color symbols

The example also contains custom-color city and town symbols:

```gdscript
"custom_color_mode": 1,
"custom_colors": [ Color(...), Color(...), Color(...) ],
"texture": "res://sprites/symbols/custom_colors/s1_city"
```

Here the three colors are a channel palette for a sprite that supports it. The
same palette also appears in `theme.symbol_custom_colors`. Keep the number and
order of `custom_colors` intact; it is not a general-purpose arbitrary color
list. The example also shows `z_index: 4`, demonstrating that symbols need not
all use the default draw order of zero.

## Boxes and their embedded images

`boxes` is an array of Godot `NinePatchRect` objects. The example's first entry
is `Object("NinePatchRect", { ... })`, with anchor, margin, stretch, visibility,
and patch-border properties. Important fields include `margin_left`,
`margin_top`, `margin_right`, `margin_bottom`, `patch_margin_*`, `modulate`,
`rect_scale`, `region_rect`, and `texture`.

The `texture` property is an `Object("ImageTexture", { ... })`. In the helper
text it contains an image placeholder, for example:

```gdscript
"texture": Object("ImageTexture", {
  "flags": 2,
  "image": ".image.png",
  "size": Vector2(466, 466)
})
```

The embedded image is associated with the nested data path:

```text
boxes.<index>.properties.texture.properties.image
```

For example, the first box uses
`boxes.0.properties.texture.properties.image`. The displayed `.image.png` is a
placeholder, not an ordinary filename. Use the helper's embedded-image
export/replace workflow; manually editing the packed image bytes is error-prone.

## Labels

`labels` is an array of text records. The example has the fields below:

| Field | Meaning |
| --- | --- |
| `text`, `font`, `size` | Label content, Wonderdraft font name, and font size. |
| `position`, `rotation`, `curve`, `align` | Placement and text layout. |
| `color`, `outline_color`, `outline_size` | Text fill and outline appearance. |
| `glow_color`, `glow_size` | Optional glow appearance. |
| `extra_spacing_char` | Per-character spacing adjustment. |
| `z_index` | Draw order. |

`theme.label_presets` is separate from `labels`: it defines named preset
settings such as `City`, `Town`, and `Water`, while each saved label carries its
own explicit appearance fields. Changing a preset does not retroactively prove
that every label will use it.

## Paths

`paths` is an array of road/path dictionaries. The example has two paths and
uses these fields:

```gdscript
{
  "color": Color(1, 0.851562, 0.851562, 1),
  "noise_seed": 1573831625,
  "points": "[ Vector2(142.667, 337.5), ... ]",
  "position": Vector2(0, 0),
  "roughness": 0.33,
  "straight": false,
  "style": "res://textures/paths/path_circle",
  "width": 4.0,
  "z_index": 0
}
```

`points` is deliberately a **quoted string** in this map, despite containing a
list-like sequence of vectors. Keep both the outer quotes and inner brackets.
`noise_seed`, `roughness`, and `straight` affect the rendered treatment of the
path; `style` points to the path texture/style resource.

## Territories

Territories are one level deeper than paths:

```text
territories.territories.<index>
```

The example records use `color`, `opacity`, string-encoded `points`, `position`,
`smoothing`, `style`, `width`, and `z_index`. Territory `style` values use
border resources such as `res://textures/borders/border_dash` and
`res://textures/borders/border_solid`. `opacity` applies to the filled area;
the other fields describe its border/geometry.

## Theme, frame, and water settings

The example's `theme` dictionary includes:

- ground and water palettes: `ground_colors`, `water_colors`, and their names;
- texture names: `ground_texture` and `water_texture`;
- coast/freshwater colors and coastline controls;
- `color_grading`, which contains `PoolColorArray`, `PoolRealArray`, and a
  strength value;
- label presets and `symbol_custom_colors`; and
- display controls such as `vignette_strength` and water hue/saturation/value.

At top level, `frame` is a dictionary with `enabled`, `size`, `texture`, and
`tint`; the example sets `grid` and `trace` to `null`. `water_flip_x`,
`water_flip_y`, `water_level`, `water_offset`, and `water_stain` are separate
top-level water controls. Preserve all related fields together when copying a
theme between maps.

## Other important sections

| Section | Text shape | Contents |
| --- | --- | --- |
| `mask`, `ground`, `water_tint` | helper image placeholder | Full-map embedded raster layers. |
| `labels` | array of dictionaries | Text, font, position, colors, outline/glow, size, rotation, and draw order. |
| `paths` | array of dictionaries | Roads/paths, including string-encoded points, width, color, style, noise, and draw order. |
| `territories.territories` | array of dictionaries | Territory areas/borders with fields similar to paths. |
| `theme` | dictionary | Palettes, texture names, coastline/render settings, and label presets. |
| `layers` | dictionary | Layer names, visibility, locking, and enabled state. |

In observed maps, a path or territory's `points` field can be a **string** that
contains a bracketed list of `Vector2(...)` values. Keep its quoting and inner
syntax intact unless you are deliberately converting its geometry.

## Editing safely

1. Keep a backup and save the edited map under a new filename.
2. Change only fields whose meaning you understand.
3. Keep commas, quotes, brackets, and braces balanced.
4. Preserve unknown keys and complete object records.
5. Use the helper's image panel for `Image` objects instead of pasting image
   bytes into the text.
6. Reopen the saved result in Wonderdraft to verify both loading and rendering.
