# Source-code guide

This guide is a developer map of the repository. It explains the responsibility
of each source file and the normal route a map takes through the application.
It complements the user-facing [README](README.md) and the detailed Rust notes
in [RUST_PORT_README.md](RUST_PORT_README.md).

## Architecture at a glance

```text
.wonderdraft_map
    │
    ▼
gcpf.rs ── decompresses the GCPF/FastLZ container ──► temporary Variant file
    │                                                    │
    │                                                    ▼
    │                                               variant.rs
    │                                                    │
    ▼                                                    ▼
save_map ◄── variant.rs ◄── images.rs / svg.rs ◄── godot_text.rs
    │                                    │                 │
    ▼                                    ▼                 ▼
.wonderdraft_map                       assets.rs        main.rs GUI editor
                                      settings.rs
```

The reusable code is in `src/lib.rs` and its modules. `src/main.rs` is the
desktop application, which coordinates those modules but does not contain the
low-level file-format implementation.

## Rust source files

| File | Responsibility | Key relationships |
| --- | --- | --- |
| [src/main.rs](src/main.rs) | Eframe/egui desktop application: windows, menus, dialogs, background workers, text editor, image previews, setup wizard, and file actions. | Calls the library modules to open/save maps, exchange SVG, manage images, resolve assets, extract PCK files, and install fonts. `App` is the main UI state. |
| [src/lib.rs](src/lib.rs) | Library entry point. Declares public modules and re-exports the common `Error`, `Result`, `Value`, and `ByteSource` types. | Lets `main.rs` and tests use the format and conversion code as one crate. |
| [src/error.rs](src/error.rs) | Application error type and `Result<T>` alias. Adds path context to I/O errors through `IoContext::at`. | Used by every fallible library module so errors have a consistent form. |
| [src/value.rs](src/value.rs) | In-memory model of Godot Variant values: dictionaries, arrays, vectors, objects, pooled arrays, and byte arrays. `ByteSource` can keep large byte data on disk. | The central data structure consumed and produced by `variant.rs`, `godot_text.rs`, `images.rs`, and `svg.rs`. |
| [src/variant.rs](src/variant.rs) | Reads and writes Godot's binary Variant representation. Also calculates encoded size and saves a map back into a GCPF container. | Decodes the decompressed map into `Value`; serializes the edited/restored `Value` when saving. Uses `gcpf.rs` and `value.rs`. |
| [src/gcpf.rs](src/gcpf.rs) | Handles Wonderdraft's GCPF wrapper: header, block table, decompression, streaming writes, and trailer validation. | Uses `fastlz.rs` for each data block. Called before Variant decoding and while saving. |
| [src/fastlz.rs](src/fastlz.rs) | Dependency-free FastLZ compressor and decompressor. Includes a literal-block writer for uncompressed GCPF output. | Internal codec used only by `gcpf.rs`. |
| [src/godot_text.rs](src/godot_text.rs) | Lexer, parser, and formatter for the human-editable Godot-like text shown in the editor. | Converts between editor text and `Value`; `main.rs` parses before save/export/import. |
| [src/images.rs](src/images.rs) | Finds embedded `PoolByteArray` images, replaces them with editable placeholders, restores them on save, imports/replaces images, creates thumbnails, exports PNG, and manages temporary cache directories. | Keeps large embedded payloads disk-backed through `ByteSource`; operates on `Value` trees. |
| [src/svg.rs](src/svg.rs) | Wonderdraft-to-SVG export and SVG-to-Wonderdraft import. Covers map layers, geometry, labels, symbol transforms, path styles, metadata, colors, and SVG record attributes. | Reads and changes `Value` map records. Uses `assets::Resolver` to calculate symbol image geometry and locate textures. |
| [src/assets.rs](src/assets.rs) | Resolves Wonderdraft texture paths such as `user://assets/` and `res://sprites/` to files. Reads `.wonderdraft_symbols` metadata for dimensions, offsets, radius, and draw mode. | Used by SVG conversion and off-canvas symbol cleanup; builds paths from `Settings`. |
| [src/settings.rs](src/settings.rs) | Persistent editor settings, platform default folders, Wonderdraft `config.ini` parsing, recent-map merging, cache sizing, and cache cleanup. | `main.rs` loads/saves it; `assets.rs` reads its asset-directory settings. |
| [src/pck.rs](src/pck.rs) | Finds and extracts `Wonderdraft.pck`, including conversion of `.wonderdraft_image` entries to PNG-named files and safe output paths. | Run from the setup/settings flow in `main.rs`; extracted sprites become the default asset source. |
| [src/fonts.rs](src/fonts.rs) | Discovers core/custom font files, reads internal font names, maintains `wonderdraft_font_names.txt`, and installs selected fonts for the current user on each supported OS. | Used by the setup wizard. SVG export uses the resulting name mapping for label fonts. |

## Typical map workflow

1. `main.rs` starts a worker after the user opens or drops a map.
2. `gcpf::decompress_file` expands `.wonderdraft_map` into the cache.
3. `variant::decode_file` parses that binary data into a `Value` tree.
4. `images::prepare` moves embedded images and other large binary fields behind
   editable placeholders, then `godot_text::format` produces the text editor
   content.
5. The user can edit text directly, export selected records with `svg::export`,
   or import changes with `svg::import`.
6. On save, `godot_text::parse` reads the text back to `Value`, `images::restore`
   restores binary payloads, and `variant::save_map` writes the Variant data
   through `gcpf::Writer`.

## Other repository files

| File or folder | Purpose |
| --- | --- |
| [wd_record_decoder.py](wd_record_decoder.py) | Small standalone Tkinter helper for decoding `wd:record` values from SVG metadata into Godot text. It does not depend on the Rust GUI. |
| [Cargo.toml](Cargo.toml) | Rust package metadata, Rust version, dependencies, GUI backend features, and release-profile settings. |
| [wonderdraft_font_names.txt](wonderdraft_font_names.txt) | Editable mapping from Wonderdraft font labels to installed font family/style information. Updated by font discovery without replacing existing mappings. |
| [start_miracledraft_map_helper.sh](start_miracledraft_map_helper.sh) and [start_miracledraft_map_helper.bat](start_miracledraft_map_helper.bat) | Platform launch helpers for Miracledraft Map Helper. |
| [install-linux-launcher.sh](install-linux-launcher.sh), [miracledraft-map-helper.desktop](miracledraft-map-helper.desktop), and the PNG/SVG icon files | Install and define the Linux desktop launcher and its icons. |
| [wiki/](wiki/) | User documentation: setup, settings/troubleshooting, keyboard shortcuts, SVG editing, and round-trip details. |
| [tempfiles/](tempfiles/) | Example and scratch SVG/map data, useful for manual interoperability checks; not core runtime source. |
| [screenshots/](screenshots/) | Images used by the README and documentation. |
| [RELEASING.md](RELEASING.md), [CONTRIBUTING.md](CONTRIBUTING.md), [TEST_REPORT.md](TEST_REPORT.md) | Release process, contribution instructions, and validation evidence. |

## Where to start when changing something

- **Map binary format or save/load behavior:** start in `gcpf.rs`, `fastlz.rs`,
  `variant.rs`, and `value.rs`.
- **Editable text syntax or validation:** start in `godot_text.rs`.
- **Embedded image import/export or preview:** start in `images.rs`.
- **SVG output or SVG round-trip fidelity:** start in `svg.rs`; inspect
  `assets.rs` too for sprite dimensions and metadata.
- **Wonderdraft discovery, cache behavior, or persisted options:** start in
  `settings.rs` and the Settings/setup-wizard portions of `main.rs`.
- **Controls, dialogs, layout, or background UI work:** start in `main.rs`.
- **Core sprite extraction or font installation:** start in `pck.rs` or
  `fonts.rs`, respectively.

The Rust modules include focused unit tests near the bottom of their files.
Run all of them with `cargo test` after changing format, parser, image, SVG, or
settings behavior.
