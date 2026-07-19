# Settings and Troubleshooting

## Stored settings

`miracledraft_map_helper.config` stores setup completion, Wonderdraft and asset folders,
cache options, and the last four maps opened by this editor. A config file in
the working directory takes priority; otherwise the editor uses one beside the
executable.

The Wonderdraft user-data folder is the directory containing `config.ini`.
Reading it allows the editor to use `recently_opened`, `last_directory`, and
`custom_assets_directory`.

## Asset resolution

- Built-in `res://sprites/...` references resolve below **Default sprites
  folder**.
- `res://packs/<pack>/sprites/...` references resolve from extracted pack data
  and custom assets, including files whose extracted image extension is `.png`.
- **Locate and extract Wonderdraft core assets…** detects or prompts for
  `Wonderdraft.pck`, extracts it into `wonderdraft_files`, and saves the sprites
  path automatically.

## Cache behavior

Large map payloads are unpacked into the configured cache while a map is open.
**Clear cache now** preserves the active map's working directory. **Clear the
cache when the program exits** is enabled by default. Extracted core files under
`wonderdraft_files` are assets, not cache, and are not removed automatically.

## Common problems

### `Wonderdraft.pck` is not detected

Check capitalization and location. Linux commonly uses
`/opt/Wonderdraft/Wonderdraft.pck`; filenames are case-sensitive. Select the
file manually in the setup wizard or set `WONDERDRAFT_PCK` to its complete path
before starting the editor.

### Symbols are missing from SVG

Open **Settings…** and verify both asset folders. Extract core assets for
built-in sprites and select or auto-locate the Wonderdraft custom-assets folder
for pack sprites. Export again and check the missing-sprite count.

### File chooser or drag and drop fails on Linux

Start the app with `start_miracledraft_map_helper.sh`. The Linux launcher uses
X11 for native file-dialog and drag-and-drop compatibility.

### A saved map does not open correctly

Keep **Verify save** enabled, always use **Save map as…**, and preserve an
untouched original. Validation proves that the written map decodes into a Godot
dictionary, but a final visual check in Wonderdraft is still required.

### Fonts do not match

Run the wizard's font scan and installation step. It updates the editable,
tab-separated `wonderdraft_font_names.txt` mapping and can install `.ttf`,
`.otf`, `.ttc`, and `.otc` files for the current user. Restart applications
that were already running if newly installed fonts are not visible.

### Inkarnate conversion fails

The converter requires an Inkarnate v3 JSON backup containing scene-size data.
PNG exports, project links, and other JSON formats are not interchangeable with
that backup. Save the generated SVG to a writable folder. Only data understood
by the converter is recovered; missing artwork may need to be rebuilt in
Wonderdraft.

### The rendered map is cropped or the scale is wrong

Open **Render settings…** and inspect the source viewport and selection area.
Reset the selection to the source viewport, or resize it to the intended crop,
then choose the output preset and orientation. Save the settings JSON once the
coordinate mapping is correct.

### A symbol is not available in the gallery

Confirm that the default sprites folder or custom asset folder is configured in
**Settings…**. Extract `Wonderdraft.pck` for built-in symbols, and check that
custom packs are located under the configured assets directory. The gallery
only displays assets the helper can resolve; it does not include or distribute
Wonderdraft assets.
