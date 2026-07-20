# Inkarnate and CSV Import

These workflows create a new Wonderdraft map. They open the same renderer used
by **Render SVG…**, where source data is assigned to translation categories.
See [Translation Settings](translation%20settings) for the category reference.

## Inkarnate backup

The helper accepts either an uncompressed Inkarnate v3 JSON backup or a
gzip-compressed `.ink` export. Click **Inkarnate → SVG…** or drag a supported
backup onto that toolbar button. When a matching file is dragged over the app,
the button turns green and reads **Drop Inkarnate backup here**.

After selecting the backup, choose one of the two actions:

1. **Export Inkarnate file to SVG…** writes a layered SVG to a location you
   choose. Open it later with **Render SVG…**.
2. **Inkarnate to .wonderdraft_map…** creates a temporary SVG internally and
   opens the renderer immediately. No intermediate SVG needs to be kept.

![Inkarnate import choice](https://raw.githubusercontent.com/Glumbosch/miracledraft_map_helper/main/screenshots/import%20inkarnate.jpg)

The conversion recovers the preview, island mask, paths, text, and grid that
are present in the backup. It does not recreate every Inkarnate asset or
effect. Set the generated `Preview` row as a tracing image, then map vector
rows such as paths and text to editable Wonderdraft categories.

## CSV, TSV, and text tables

Choose **Render from CSV…** or drop a `.csv`, `.tsv`, or `.txt` file onto that
button. The importer detects common delimiters, reads UTF-8 with a
Windows-1252 fallback, and shows a column-mapping window before the renderer.

![CSV column mapping](https://raw.githubusercontent.com/Glumbosch/miracledraft_map_helper/main/screenshots/csv_file_import_settings.jpg)

Map the columns that are present in the table:

- **Tag** identifies the element type, such as `path`, `polyline`, `line`, or a
  point-like element.
- **ID** groups and identifies records.
- **Name / label** and **Label content** supply names and label text.
- **Class** separates rows into renderer rows that can receive different
  translation settings.
- **Fill**, **Stroke**, and **Stroke width** supply optional SVG-style values.
- **Coordinates** supplies point coordinates or path geometry.

For paths, enable **first pair is the absolute origin and remaining pairs are
offsets** only when the table stores relative segments after its first point.
Use **Fit to data** to make the source viewport cover the imported coordinates,
then choose the Wonderdraft output size. Click **Continue to render settings**
to assign categories, preview the result, and save the new map.

Rows tagged `path`, `polyline`, or `line` remain open paths even when a fill
column exists.
