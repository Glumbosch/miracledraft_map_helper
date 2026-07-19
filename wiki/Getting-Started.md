# Getting Started

## Install and launch

Download the archive for Linux, Windows, or Apple Silicon macOS from the
[latest release](https://github.com/Glumbosch/miracledraft_map_helper/releases/latest).

- Linux: extract the archive and run `./miracledraft-map-helper`. Running
  `./install-linux-launcher.sh` adds an application-menu entry for the current
  user.
- Windows: extract the ZIP and run `miracledraft-map-helper.exe`.
- macOS: extract the archive and run `miracledraft-map-helper` from Terminal. The
  unsigned binary may need approval in Privacy & Security on first launch.

## First-start wizard

The five-step wizard opens on first launch. Every Wonderdraft integration step
is optional and can be configured later from **Settings…**.

1. **Welcome** explains what will be configured.
2. **Wonderdraft user data** locates the folder containing `config.ini`. This
   supplies recent maps and the custom-assets location.
3. **Wonderdraft core sprites** detects or lets you choose `Wonderdraft.pck`,
   extracts it into `wonderdraft_files`, and saves the extracted `sprites`
   directory in the editor settings.
4. **Wonderdraft fonts** scans core and custom asset fonts, updates
   `wonderdraft_font_names.txt`, and optionally installs selected fonts for the
   current user. Existing identical files are skipped; same-name conflicts are
   preserved and reported.
5. **Cache and summary** selects the disk-cache directory and whether map cache
   data is cleared when the editor exits.

Use **Back** and **Next** to move through the wizard. **Skip font installation**
jumps to the summary. **Finish setup** saves the choices. Reopen the wizard at
any time with **Settings… → Run setup wizard…**.

## Open a map

Use **Open map**, select an available entry under **Open recent**, or drag a
single `.wonderdraft_map` file onto the window. Loading and native file choosers
run in the background so the interface stays responsive.

The editor decodes the map into editable Godot text and keeps large embedded
images disk-backed in the configured cache. The loaded map is added to the four
most recent files remembered by the editor.

## Safe first workflow

1. Keep an untouched copy of the original `.wonderdraft_map`.
2. Open the working copy.
3. Make a small text or SVG change.
4. Leave **Verify save** enabled and use **Save map as…** so the original is not
   overwritten.
5. Open the new map in Wonderdraft and inspect the result.

## Inkarnate workflow

Choose **Inkarnate → SVG…**, select an Inkarnate v3 JSON backup, and select an
SVG destination. The native converter recovers the preview image, land mask,
paths, text, and grid into named SVG layers. Open that SVG with **Render SVG…**.
To keep the original artwork visible while you configure the conversion, select
the `Preview` layer and enable **Use this layer/class as tracing image**.

This converts recoverable map data, not every Inkarnate asset and effect. The
preview is useful as a visual guide; paths and labels can be rendered as
editable Wonderdraft content, while textures and asset placement may need to
be recreated.

## Render an SVG or table as a new map

Use **Render SVG…** for an SVG from Inkarnate or another source. Use **Render
from CSV…** for coordinate data such as roads, points, polygons, or labels.
Both workflows open a separate renderer window:

1. Select the source file.
2. Choose the imported layers/classes or map-data columns to render.
3. Open **Render settings…** and choose the output size, orientation, source
   selection, and per-class translation/fill settings.
4. Preview the result and optionally save the settings as JSON.
5. Render the new `.wonderdraft_map`, then open it in Wonderdraft and check it.

For an SVG, assign text to the **label** category to create Wonderdraft
labels. For CSV input, map a column to **Label content**. Open paths remain
open when CSV rows are tagged as `path`, `polyline`, or `line`.
