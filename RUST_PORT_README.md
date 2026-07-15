# Wonderdraft Map Editor — Rust port

This directory now contains a native Rust port of `wonderdraft_gui_memory.py`.
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
- Persist asset and disk-cache settings in `wonderdraft_gui.config` beside the
  executable.

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
