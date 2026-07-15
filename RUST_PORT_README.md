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

## Ported workflows

- Open and decode GCPF/FastLZ `.wonderdraft_map` files.
- Edit and validate the complete Godot Variant text representation.
- Save compressed or literal-only maps, with optional decode verification.
- List, preview, export, and replace embedded images.
- Export labels, symbols, paths, and the mask background to SVG.
- Import edited SVG labels, symbols, and paths back into the editable map.
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
