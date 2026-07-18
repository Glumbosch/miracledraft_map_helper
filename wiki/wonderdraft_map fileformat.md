# Wonderdraft `.wonderdraft_map` file format

> **Unofficial, reverse-engineered documentation.** The format is not a
> published Wonderdraft API and fields can change between Wonderdraft versions.
> Always keep an untouched copy of a map, and test a rewritten map in
> Wonderdraft.

A `.wonderdraft_map` file has two binary layers:

```text
.wonderdraft_map
  GCPF container (FastLZ-compressed blocks)
    Godot binary Variant stream
      one top-level Dictionary: the map record
```

It is **not JSON**. Miracledraft Map Helper displays the decoded Variant as a
Godot-like text syntax so that it can be inspected and edited. That text is a
convenience representation; the file saved by Wonderdraft is binary.

For a field-by-field guide to that readable text, including symbols, boxes, and
map-data paths, see [Map-data text syntax](Map-Data-Text-Syntax).

## 1. GCPF container

The outer container begins and ends with the ASCII magic `GCPF`. All integer
fields are unsigned 32-bit little-endian values.

| Offset | Size | Meaning |
| --- | ---: | --- |
| `0x00` | 4 | Magic: `GCPF` |
| `0x04` | 4 | Mode. Observed maps use `0`. |
| `0x08` | 4 | Uncompressed block size. |
| `0x0c` | 4 | Total uncompressed byte length. |
| `0x10` | `4 × block_count` | Compressed-size table: one length for each block. |
| after table | variable | FastLZ payloads, in table order. |
| end | 4 | Trailer magic: `GCPF`. |

`block_count` is `uncompressed_size / block_size + 1` using integer division.
The final entry may therefore be an empty block when the uncompressed size is
an exact multiple of the block size. Concatenate the decompressed blocks to
obtain the Variant stream. The size table and trailing magic make it possible
to validate the container before parsing map data.

## 2. Godot binary Variant stream

The decompressed payload starts with a four-byte little-endian length prefix.
It is the number of bytes that follow it; the prefix itself is not included.
Those remaining bytes encode exactly one Godot Variant, normally a Dictionary.

Every Variant begins with a little-endian 32-bit header. Its low byte is the
type ID; bit `0x00010000` requests a 64-bit integer or real where supported.
Strings are UTF-8 and are padded to a four-byte boundary. Dictionaries and
arrays store a 32-bit element count followed by their values.

Frequently encountered types include:

| Text form | Meaning in the binary Variant |
| --- | --- |
| `null`, `true`, `false` | Nil and Boolean |
| `42`, `0.5` | 32-bit or flagged 64-bit integer/real |
| `"text"` | UTF-8 String, length-prefixed and four-byte aligned |
| `{ ... }`, `[ ... ]` | Dictionary and Array |
| `Vector2(x, y)`, `Color(r, g, b, a)` | Godot fixed-size math values, stored as `f32` components |
| `Object("Image", { ... })` | Godot Object: class name plus named properties |
| `PoolByteArray(...)` | Length-prefixed byte payload, commonly image pixels |
| `PoolRealArray(...)`, `PoolColorArray(...)` | Packed numeric/color arrays |

The following is **decoded display syntax**, not JSON and not a byte-for-byte
dump of the file:

```gdscript
{
  "map_width": 1024,
  "map_height": 768,
  "symbols": [
    {
      "position": Vector2(66, 422.5),
      "scale": Vector2(0.35, 0.35),
      "rotation": 0.0,
      "texture": "res://sprites/..."
    }
  ]
}
```

## 3. Top-level map dictionary

The top-level Dictionary contains map dimensions, rendering/theme options,
layer settings, vector records, resource references, and embedded raster data.
The exact key set is version- and map-dependent. Common keys are:

| Key | Typical value | Purpose |
| --- | --- | --- |
| `version` | integer | Wonderdraft map-format version. Preserve it unless you know a target version requires a change. |
| `map_width`, `map_height` | integer | Canvas dimensions in map pixels. |
| `mask`, `ground`, `water_tint` | Image object | Full-map embedded raster layers. |
| `symbols`, `labels`, `paths`, `boxes`, `windroses` | Array | Placed/drawn map records. |
| `territories` | Dictionary | Territory settings and, typically, a nested `territories` array. |
| `theme` | Dictionary | Palette, texture, coastline, label-preset, and rendering settings. |
| `layers` | Dictionary | Layer names, lock state, visibility, and enabled flag. |
| `included_packs`, `included_default_packs` | Array | Pack identifiers associated with the map. |
| `frame`, `grid`, `trace` | Dictionary or `null` | Optional display/reference features. |

Unknown keys should be retained during a read-modify-write operation. Removing
fields merely because they are not understood can change rendering or make a
map incompatible with the originating Wonderdraft version.

## 4. Embedded images

Embedded images are not PNG files embedded verbatim. They are Godot `Image`
objects whose `data` property holds a Dictionary similar to:

```gdscript
Object("Image", {
  "data": {
    "width": 1024,
    "height": 768,
    "format": "RGBA8",
    "mipmaps": false,
    "data": PoolByteArray(... raw pixels ...)
  }
})
```

The packed byte array is raw pixel data in the declared image format. A tool
must preserve the width, height, format, mipmap flag, and byte length together;
changing only the byte array is not a safe image replacement. The helper
exports these objects as PNG for editing, then creates a compatible `Image`
object when importing a replacement.

Important image locations include:

| Path in decoded map | Meaning |
| --- | --- |
| `mask` | Full-canvas land/water mask. In normal RGBA exports, opaque black is land and red marks inland water; transparent pixels are ocean/background. Preserve the alpha and color channels when editing it. |
| `ground` | Full-canvas ground paint/tint layer. |
| `water_tint` | Full-canvas water tint layer. |
| `boxes.<index>.properties.texture.properties.image` | Image used by a placed nine-patch box. |

The visual meaning of the ground and water layers also depends on the map's
theme and rendering settings, so they should be treated as paint data rather
than as self-contained final artwork.

## 5. Vector and placed records

### Symbols

`symbols` is an array of Dictionaries. A typical record has `position`,
`scale`, `rotation`, `mirror`, `texture`, `type`, and `z_index`, with optional
paint/color and outline fields:

```gdscript
{
  "position": Vector2(66, 422.5),
  "scale": Vector2(0.35, 0.35),
  "rotation": 0.0,
  "mirror": false,
  "texture": "user://assets/my_pack/sprites/mountains/volcano",
  "type": "mountain",
  "z_index": 0
}
```

`texture` is a resource path, not an embedded image. Common forms are
`res://sprites/...` for core resources, `res://packs/...` for bundled/default
packs, and `user://assets/...` for installed custom packs. The file extension
may be absent in the map record. The actual resolved file and supported fields
depend on the installed Wonderdraft assets and `.wonderdraft_symbols` metadata.

For paintable symbols, `radius`, `offset`, and `sample` may be present. `offset`
is a `Vector2` relative to the sprite's centre; `radius` describes the painted
area; and `sample` is usually a `Color(r, g, b, a)` used for the sampled tint.
`type` is the symbol category such as `mountain`, not invariably the literal
string `symbol`.

### Labels

Each `labels` entry normally stores text, position, font, size, alignment,
rotation, curve, fill color, outline color/size, glow color/size, character
spacing, and z-index. Font names are labels understood by Wonderdraft; they
are not guaranteed to be installed on another machine.

### Paths and territories

`paths` is an array of road/path records. A record commonly includes `points`,
`width`, `color`, `style`, `smoothing`, `position`, and `z_index`. In observed
maps, `points` can itself be a string containing Godot-style `Vector2` values,
so it must not be assumed to be a native Variant array.

Territory records usually live at `territories.territories` and use similar
point, width, color, opacity, style, smoothing, position, and z-index fields.
The surrounding `territories` Dictionary can contain additional territory
configuration, so retain it instead of replacing it with only the array.

### Boxes, frames, grids, and theme

`boxes` records are Godot `NinePatchRect` objects. Their nested properties
include margins, patch margins, modulation/opacity, region data, and a nested
`ImageTexture` containing the embedded image described above. They are richer
than a plain rectangle, so copying the complete record is safer than creating
one field by field.

`frame`, `grid`, `layers`, and `theme` are configuration dictionaries. The
theme can include ground and water palettes, named label presets, texture names,
coastline styling, and color-grading settings. Their field names should be
treated as data, not as a stable programming interface.

## 6. Safe editing rules

1. Work from a copy and save to a new filename.
2. Decode the GCPF container before trying to interpret the data as a Variant.
3. Keep the Variant length prefix, every type header, string padding, and image
   metadata consistent when writing.
4. Preserve unknown keys, object properties, and packed arrays.
5. Resolve `res://` and `user://` paths against the target installation before
   assuming a symbol texture will appear.
6. Reopen the output in Wonderdraft; a successful decode by another tool is not
   a guarantee that the map renders as intended.

Miracledraft Map Helper implements this sequence directly: it decompresses the
GCPF blocks, decodes the Variant tree, keeps embedded byte arrays separate while
editing, then restores the tree and writes it back through GCPF on save.
