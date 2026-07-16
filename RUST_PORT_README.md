# Wonderdraft Map Editor — Rust

The main installation, setup, workflow, and troubleshooting guide is now
[`README.md`](README.md). This document records additional implementation notes
for the native Rust port.

It keeps embedded image buffers disk-backed while maps are open and while they
are written back to Godot's GCPF/FastLZ container.

## Build and run

```bash
cargo run --release
```

The release executable is `target/release/wonderdraft-editor` (or
`wonderdraft-editor.exe` on Windows).

On Linux the application is compiled with Eframe/Winit's X11 backend only.
Wayland window support is deliberately disabled because native file drag and
drop is required. The Linux launcher also sets `GDK_BACKEND=x11` for native
helper UI.

## Ported workflows

- Open and decode GCPF/FastLZ `.wonderdraft_map` files.
- Open maps by dropping a `.wonderdraft_map` file anywhere on the main window.
- Keep the main window responsive while native file/folder choosers are open;
  map decompression and decoding also run on a background worker.
- Edit and validate the complete Godot Variant text representation.
- Save compressed or literal-only maps, with optional decode verification.
- List, preview, export, and replace embedded images.
- Export labels, symbols, paths, and the mask background to SVG.
- Import edited SVG labels, symbols, and paths back into the editable map.
- Choose SVG layers independently with **Background mask**, **Roads / paths**,
  **Symbols**, and **Labels** checkboxes.
- Resolve custom `user://assets/` and default `res://sprites/` assets.
- Locate and extract core sprites from `Wonderdraft.pck` in the background.
- Read Wonderdraft's `config.ini` for its recent maps, last-used map folder,
  and optional `custom_assets_directory`.
- Show the cache size, clear stale cache data on demand, and clear the cache on
  exit by default.
- Persist asset and disk-cache settings in `wonderdraft_gui.config` beside the
  executable.

## Settings and Wonderdraft integration

The first-start wizard explains and configures the Wonderdraft user-data folder,
core sprites, and disk cache. Open **Settings… → Run setup wizard…** to run it
again. The same settings can also be edited directly in the Settings window.

The editor checks the standard user-data location for the current operating
system. Automatic custom-asset discovery uses the folder's `assets` directory,
or the `assets` directory below the path from `custom_assets_directory` when
Wonderdraft has that setting.

The toolbar's **Open recent** menu mirrors Wonderdraft's `recently_opened`
entries. **Open map** starts in Wonderdraft's `last_directory`.

## Extracting Wonderdraft core assets

Use the setup wizard, or open **Settings…** and select **Locate and extract
Wonderdraft core assets…**. The editor checks Wonderdraft's standard
installation paths for the current operating system and recognizes both
`Wonderdraft.pck` and `wonderdraft.pck`. Current Linux installations commonly
use `/opt/Wonderdraft/Wonderdraft.pck`. If the pack is not found, the editor
opens a file chooser labelled **Wonderdraft.pck**. Set the `WONDERDRAFT_PCK`
environment variable to use another path automatically.

Files are unpacked to `wonderdraft_files` beside the working application, every
`.wonderdraft_image` file is written with a `.png` extension, and the extracted
`wonderdraft_files/sprites` directory is saved as the default sprites folder.

The Rust modules are deliberately separated so the binary format and SVG code
can be tested without launching the GUI. Run the test suite with:

```bash
cargo test
```

As with the Python tool, keep an untouched backup of maps and test SVG-imported
paths in Wonderdraft, because path record schemas vary between releases.

## Updated symbol geometry

SVG interchange format version 2 matches Wonderdraft's sprite geometry more
closely. Symbol images use their intrinsic dimensions multiplied by the record
scale. `.wonderdraft_symbols` metadata supplies logical radius and source-pixel
offsets; mirroring happens around the visible sprite center and rotation happens
around the stored map position. The importer reverses this geometry so an
unchanged SVG round-trips position, scale, rotation, mirror, and offset.

Non-image disk-backed `PoolByteArray` values are represented in editable text as
compact `PoolByteArrayRef(path, length)` placeholders and restored byte-for-byte
when saving. Non-image Godot objects likewise retain explicit
`Object(class, properties)` syntax.
