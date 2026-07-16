# Wonderdraft Map Editor 

Experimental desktop editor for Wonderdraft `.wonderdraft_map` files. 

For current build instructions, the first-start setup wizard, core-pack
discovery, and troubleshooting, see [`README.md`](README.md). This document
focuses on the detailed map and SVG workflows.

## Main features

- Opens and decodes `.wonderdraft_map` files directly.
- Edits the complete Godot Variant text representation.
- Exports and replaces the three embedded map images as PNG.
- Exports labels, symbols, paths, and territories to an editable SVG.
- Imports edited SVG text, symbol images/circles, paths, and territory points
  back into the map.
- Resolves `user://assets/...` custom textures and `res://sprites/...` default textures.
- Saves a new FastLZ-compressed `.wonderdraft_map` and verifies it by decoding it again.

## Installation requirements

- A Rust toolchain to build the application.
- A Wonderdraft installation to extract and resolve the built-in sprites.

## Asset folders

Use the first-start wizard or **Settings…** to configure:

1. **Custom asset folder** — normally:

Linux
   ```text
   /home/<username>/.local/share/Wonderdraft/assets
   ```

2. **Default sprites folder** — the `sprites` directory extracted from
   `Wonderdraft.pck`, normally:

   ```text
   <application working directory>/wonderdraft_files/sprites
   ```

The program checks common Windows, Linux, and macOS locations automatically and
recognizes both `Wonderdraft.pck` and `wonderdraft.pck`. On Linux it checks
`/opt/Wonderdraft/Wonderdraft.pck`. Settings are saved as JSON in:

```text
wonderdraft_gui.config
```

The application has its own PCK extractor. Use **Choose Wonderdraft.pck…** in
the wizard if automatic discovery does not find the installation.

## SVG export

Click **Export SVG…** after opening a map.

The SVG contains:

- The embedded Wonderdraft `mask` PNG as a full-size background image.
- Wonderdraft labels as real SVG `<text>` elements.
- Symbols as SVG images pointing to their source sprite files, or as embedded
  base64 PNG definitions with lightweight `<use>` clones.
- Existing paths as editable SVG polylines when their point array can be identified.
- Territories as editable SVG polygons with solid, gradient-blurred, dashed, or
  dark-dotted borders matching their Wonderdraft style.
- `wd:*` metadata attributes containing the original Wonderdraft records for reliable round trips.

When a custom or default sprite cannot be found, the exporter places a magenta-outlined SVG circle at the symbol position. The original symbol record remains attached to that circle.

Wonderdraft/Godot color components are treated as nonlinear sRGB components. Values such as `Color(1, 0, 0, 1)` therefore map directly to SVG `#ff0000`; the program does not apply a linear-light conversion.

Symbol display size is calculated using the Wonderdraft radius, the symbol `scale`, the source image dimensions, and the nearest `.wonderdraft_symbols` metadata entry when available. The radius is handled in map pixels.

## SVG import

Click **Import SVG…** to update the map data shown in the editor. Save the result afterward with **Save map as…**.

For SVG files exported by this program, the importer preserves the original Wonderdraft records and updates visible properties such as:

- Label text, position, font, size, alignment, rotation, fill, and outline.
- Symbol position, displayed size, rotation, mirroring, sample color, and texture.
- Path points and basic stroke color/width.
- Territory points while preserving the original territory record and style.

For arbitrary SVG files, the importer makes a best-effort conversion:

- Every SVG `<text>` element becomes a Wonderdraft label.
- Sprite `<image>` references are matched against the configured custom and default asset folders.
- Circles can be imported as symbols.
- Unfilled SVG paths, polylines, polygons, and lines can be imported as Wonderdraft paths.
- A full-page raster backdrop is ignored rather than imported as a symbol.

When an SVG image does not correspond to a configured custom or default texture, its imported texture becomes:

```text
res://sprites/symbols/custom_colors/s2_capital
```

To retain the most accurate round trip, keep the `wd:*` attributes attached to the exported elements when editing in Inkscape.

## Embedded PNG workflow

1. Select `ground`, `mask`, or `water_tint` in the right panel.
2. Click **Export PNG**.
3. Edit the PNG in GIMP, Krita, Photoshop, or another image editor.
4. Click **Replace PNG** and select the changed image.
5. Save the map.

Replacement images must use the original dimensions. They are converted to RGBA8.

## Important limitations

- Keep an untouched backup of every map.
- The implementation has only been tested against  Wonderdraft 1.1.8.2  release and a limited set of map features.
- Existing path records are preserved when the exporter can locate their point array. Wonderdraft path schemas can vary, so newly created, untagged SVG strokes use a conservative generic path record and should be tested in Wonderdraft.
- Curved Wonderdraft labels retain their metadata, but SVG export currently renders them as ordinary text rather than SVG text-on-path.
- Label glow is retained in metadata but is not visually reproduced in the SVG.
- External SVG symbol images depend on the configured asset files remaining available.
