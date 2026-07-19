# Functions and Keyboard Shortcuts

## Keyboard shortcuts

These are the shortcuts implemented specifically by Miracledraft Map Helper.

| Shortcut | Context | Function |
| --- | --- | --- |
| `Ctrl+F` | Anywhere in the main window | Opens Find and focuses its search field. |
| `Enter` | Find field | Finds the next exact, case-sensitive match. Search wraps to the beginning. |
| `Esc` | While Find is open | Closes Find. |

The Godot-text area is a standard multiline text editor. Platform-standard text
editing actions supplied by the GUI toolkit—such as arrow-key navigation,
selection, copy, cut, paste, undo, redo, and select all—are also available when
the text area has focus. They are not application-specific global shortcuts.

## Main toolbar

| Function | What it does |
| --- | --- |
| **Open map** | Opens a file chooser for a `.wonderdraft_map`. |
| **Open recent** | Combines the editor's recent maps with Wonderdraft's `recently_opened` list, removes duplicates, and disables files that no longer exist. The Wonderdraft `config.ini` is refreshed whenever this menu opens. |
| **Validate text** | Parses the current Godot text and confirms that its root is a dictionary. It does not save the map. |
| **Save map as…** | Rebuilds the map from the current text, embedded images, and binary data, then writes a new `.wonderdraft_map`. |
| **Export SVG…** | Exports the enabled map layers to SVG using the current embedding settings. |
| **Import SVG…** | Reads supported SVG elements and their `wd:*` metadata into the currently open map data. Save afterward to create a map file. |
| **Export map data…** | Writes the currently displayed Godot text to a plain `.txt` file. |
| **Export all PNGs** | Exports every detected embedded image to a selected directory. |
| **Settings…** | Opens Wonderdraft integration, asset-folder, core-extraction, and cache settings. |

## Render as new Wonderdraft map

**Render SVG…** and **Render from CSV…** open a separate renderer window.
Its **Render settings…** button controls output dimensions, imported
classes/layers, and source selection.

| Control | Effect |
| --- | --- |
| **Preset / Orientation** | Chooses a standard size and keeps width/height in landscape, portrait, or square form. Manual dimension edits update the orientation automatically. |
| **Source viewport** | Read-only total coordinate bounds across all imported data. |
| **Selection area** | The editable coordinate rectangle included in the output. Drag the yellow preview rectangle's corners to resize it, or drag inside it to move it. |
| **Adjust output map aspect ratio to selection** | Keeps the longer current output dimension and derives the shorter dimension from the selection area's aspect ratio. |
| **View full preview** | Opens a separate, scrollable native window at one source coordinate per pixel. |
| **Load/Save settings JSON** | Saves both per-class translation settings and the complete Render settings form. |

For raster classes, **Fill override** has a nested **No fill** checkbox. It
writes `fill:none`, which is useful for freshwater lines. CSV rows tagged as
`path`, `polyline`, or `line` remain open; they are not converted into closed
filled polygons by a CSV fill column.

## Save and SVG options

| Option | Effect |
| --- | --- |
| **Compress saved map** | Uses normal GCPF compression. Disable it to write literal-only data, which is larger but can aid low-level inspection. |
| **Verify save** | Decodes the newly written map again and checks that its root is a Godot dictionary. Keep this enabled for normal work. |
| **Embed mask in SVG** | Stores the background mask inside the SVG as a data URI. When disabled, the SVG refers to an external PNG. |
| **Embed boxes in SVG** | Stores each exported box texture inside the SVG as a data URI. When disabled, the SVG links a companion `.box-N.png` file. |
| **Embed symbols in SVG** | Embeds each distinct source symbol once and reuses it with SVG `<use>` elements, making the SVG portable without duplicating image data. |

The **SVG export layers** checkboxes independently include or exclude the
background mask, boxes, roads/paths, symbols, labels, and territories. Included
groups are written as named Inkscape layers. The importer can infer newly
created, untagged elements from these layer names when their SVG element type
matches the layer.

## Map data editor

The central editor exposes the complete decoded map in Godot text syntax. You
can edit values directly, then use **Validate text** before exporting or saving.

- **Find next** performs the same action as `Enter` in the Find field.
- **Close** performs the same action as `Esc` while Find is open.
- **Jump to section** locates and selects the first marker for **Boxes**,
  **Symbols**, **Roads / paths**, **Labels**, **Territories**, or **Theme**. If
  a section is absent, the status line reports it.
- **Remove off-canvas symbols** deletes only symbols whose complete transformed
  bounds are outside the map. The calculation includes scale, offset, rotation,
  mirroring, and outline width. It changes the text in memory; use **Save map
  as…** to write the result.

## Embedded images

The right panel lists every image payload detected in the open map. Selecting
one shows its dimensions, pixel format, raw size, storage mode, and a preview.

| Function | What it does |
| --- | --- |
| **Export PNG** | Converts the selected embedded image to PNG. |
| **Replace PNG** | Replaces the selected payload from a PNG, JPEG, or WebP file while retaining the map field's expected image structure. |
| **Export all PNGs** | Converts all detected image payloads into separate PNG files. |

Common entries include `ground`, `mask`, and `water_tint`; exact entries depend
on the map.

## Drag and drop and background work

Dragging a `.wonderdraft_map` over the window displays a drop overlay. Dropping
it opens the map. Other file types are rejected, and a second map cannot be
opened while a file chooser or map load is already active.

Map loading, file choosers, Wonderdraft core extraction, and font installation
run without freezing the main interface. A spinner and status message identify
the current operation.

## Settings

| Function | What it does |
| --- | --- |
| **Run setup wizard…** | Restarts the five-step first-run setup. |
| **User-data folder** | Points to the Wonderdraft folder containing `config.ini`. |
| **Automatically locate custom assets…** | Uses the normal `assets` folder or honors `custom_assets_directory` from `config.ini`. |
| **Reload config.ini** | Refreshes recent maps, last directory, and configured custom assets. |
| **Custom asset folder** | Manually selects custom pack resources when automatic location is disabled. |
| **Default sprites folder** | Selects extracted Wonderdraft core sprites used to resolve built-in `res://sprites/...` references. |
| **Locate and extract Wonderdraft core assets…** | Detects `Wonderdraft.pck` or prompts for it, extracts its files, and persists the resulting sprites folder. |
| **Disk cache folder** | Selects where unpacked map payloads are held while a map is open. |
| **Clear the cache when the program exits** | Removes map cache data on shutdown. Core assets are not cache data. |
| **Clear cache now** | Removes inactive cache data immediately while preserving the currently open map's working data. |
| **Save** | Validates the cache folder, reloads Wonderdraft configuration, and persists settings. |
| **Cancel** | Restores the settings that were active when the window opened. |
| **About → Build time** | Shows when the running executable was built, alongside its version number. |
