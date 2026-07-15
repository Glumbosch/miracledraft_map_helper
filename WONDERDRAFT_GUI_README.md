# Wonderdraft Map Editor — SVG edition

Experimental desktop editor for Wonderdraft `.wonderdraft_map` files. It has been tested against the supplied Wonderdraft map format version 15.

## Main features

- Opens and decodes `.wonderdraft_map` files directly.
- Edits the complete Godot Variant text representation.
- Exports and replaces the three embedded map images as PNG.
- Exports labels, symbols, and paths to an editable SVG.
- Imports edited SVG text, symbol images/circles, and paths back into the map.
- Resolves `user://assets/...` custom textures and `res://sprites/...` default textures.
- Saves a new FastLZ-compressed `.wonderdraft_map` and verifies it by decoding it again.
- Stores asset-folder settings in `wonderdraft_gui.config` beside `wonderdraft_gui.py`.

## Requirements

Python 3.10 or newer is recommended. Install Pillow:

```bash
python -m pip install pillow
```

Tkinter is included with most Windows Python installations. On Ubuntu/Debian:

```bash
sudo apt install python3-tk
```

Start the program:

```bash
python wonderdraft_gui.py
```

Windows users can double-click `start_wonderdraft_editor.bat`. Linux users can run `./start_wonderdraft_editor.sh`.

## Asset folders

Use **Asset folders…** in the toolbar to configure:

1. **Custom asset folder** — normally:

   ```text
   /home/<username>/.local/share/Wonderdraft/assets
   ```

2. **Default sprites folder** — the `sprites` directory extracted from `Wonderdraft.pck`, for example:

   ```text
   /home/wolf/code/wonderdraft_manipulator/sprites
   ```

The program tries common Windows, Linux, and macOS locations automatically. Settings are saved as JSON in:

```text
wonderdraft_gui.config
```

The file is always read from the same directory as `wonderdraft_gui.py`.

A default asset extraction command is:

```bash
./GodotPCKExplorer.Console -e "/opt/Wonderdraft/Wonderdraft.pck" "/home/username/documents/wonderdraftfiles"
```

Then select the resulting `sprites` directory in the editor.

## SVG export

Click **Export SVG…** after opening a map.

The SVG contains:

- The embedded Wonderdraft `mask` PNG as a full-size background image.
- Wonderdraft labels as real SVG `<text>` elements.
- Symbols as SVG `<image>` elements pointing to their source sprite files.
- Existing paths as editable SVG polylines when their point array can be identified.
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
- The implementation has not been tested against every Wonderdraft release and every map feature.
- Existing path records are preserved when the exporter can locate their point array. Wonderdraft path schemas can vary, so newly created, untagged SVG strokes use a conservative generic path record and should be tested in Wonderdraft.
- Curved Wonderdraft labels retain their metadata, but SVG export currently renders them as ordinary text rather than SVG text-on-path.
- Label glow is retained in metadata but is not visually reproduced in the SVG.
- External SVG symbol images depend on the configured asset files remaining available.
