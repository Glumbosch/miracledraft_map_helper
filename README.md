# Wonderdraft Map Editor

A native desktop editor for inspecting and converting Wonderdraft
`.wonderdraft_map` files. It can edit the decoded Godot data, exchange map
layers with SVG editors, and replace embedded map images without loading every
binary payload into memory.

> This is an experimental, unofficial tool. Keep an untouched backup of every
> map and test edited maps in Wonderdraft before relying on them.

## Requirements

- A Rust toolchain for building the editor.
- A Wonderdraft installation if you want to resolve and export its built-in
  sprites.
- Linux builds use X11 because native drag and drop is required.

## Build and run

Linux and macOS:

```bash
cargo run --release
```

On Linux you can also use the launcher, which selects the X11 backend:

```bash
./start_wonderdraft_editor_rust.sh
```

Windows:

```bat
start_wonderdraft_editor_rust.bat
```

The compiled executable is `target/release/wonderdraft-editor` (or
`wonderdraft-editor.exe` on Windows).

### Linux application launcher

After building the release executable, install an application-menu launcher
with:

```bash
./install-linux-launcher.sh
```

The installer copies the executable into the user application-data directory,
uses `wonderdraft_map_extractor.png` as the fallback launcher icon, installs
`wonderdraft_map_extractor.svg` as the scalable icon, and creates
`~/.local/share/applications/wonderdraft-map-extractor.desktop` (or the
equivalent location below `XDG_DATA_HOME`). No administrator access is needed.

The repository also contains `wonderdraft-map-extractor.desktop`, which launches
the release executable directly from this checkout.

## First-start setup

The setup wizard opens the first time the editor runs. It can be opened again
later with **Settings… → Run setup wizard…**.

1. Confirm the Wonderdraft user-data folder containing `config.ini`. This lets
   the editor find recent maps and custom assets. The usual Linux location is
   `~/.local/share/Wonderdraft`.
2. Extract the core sprites. If `Wonderdraft.pck` is found automatically, click
   **Extract detected Wonderdraft.pck**. Otherwise click
   **Choose Wonderdraft.pck…** and select the file manually.
3. Confirm the disk-cache folder and finish setup.

The Wonderdraft integration and sprite extraction are optional. You can finish
the wizard without them and configure them later.

### Wonderdraft.pck discovery

The editor checks both `Wonderdraft.pck` and the older lowercase
`wonderdraft.pck` spelling. On Linux, filenames are case-sensitive. Standard
search directories include:

- Linux: `/opt/Wonderdraft` and `~/Games/Wonderdraft`
- macOS: `/Applications/Wonderdraft.app/Contents/Resources`
- Windows: the `Wonderdraft` directory below the available Program Files
  locations

For a nonstandard installation, set `WONDERDRAFT_PCK` to the complete pack path
before starting the editor:

```bash
WONDERDRAFT_PCK=/another/location/Wonderdraft.pck \
  ./start_wonderdraft_editor_rust.sh
```

Extracted files are written to `wonderdraft_files` in the application's working
directory. Wonderdraft image resources are given a `.png` extension, and
`wonderdraft_files/sprites` is saved as the default sprites folder.

## Main workflows

- Open a map with **Open map**, **Open recent**, or drag and drop.
- Validate or edit the complete Godot text representation.
- Save a compressed or literal-only map and optionally verify it by decoding it
  again.
- Export or replace embedded `ground`, `mask`, and `water_tint` images.
- Export the background, roads/paths, symbols, and labels to SVG.
- Import edited SVG elements into the open map, then save the result as a new
  `.wonderdraft_map` file.

Built-in `res://sprites/...` textures resolve below the configured core
`sprites` folder. Pack textures such as `res://packs/<pack>/sprites/.../5`
resolve below the sibling `wonderdraft_files/packs` folder and automatically
pick up extracted image extensions such as `.png`.

## SVG round trip

For the most reliable round trip, keep the `wd:*` metadata attributes on SVG
elements. These attributes retain the original Wonderdraft records while the
visible SVG properties are edited. Untagged SVG content is converted on a
best-effort basis because Wonderdraft record formats can differ by version.

The layer checkboxes control whether the background mask, roads/paths, symbols,
and labels are included. **Embed mask in SVG** stores the mask as a data URI;
otherwise the SVG refers to an external PNG. **Embed symbols in SVG** writes one
base64 PNG definition for each distinct source symbol and places repeated
instances as SVG `<use>` clones, making the SVG portable without duplicating
the same image data.

## Settings and generated data

`wonderdraft_gui.config` stores the Wonderdraft, asset, cache, and completed
setup settings as JSON. The editor first uses a config file in the working
directory when one exists; otherwise it uses a file beside the executable.

Map payloads are unpacked into the configured cache while a map is open. The
cache is cleared on exit by default. Core assets in `wonderdraft_files` are not
cache data and are not removed automatically.

## Troubleshooting

**`Wonderdraft.pck` exists but is not detected**

Check the exact capitalization and location. Current Linux installations often
use `/opt/Wonderdraft/Wonderdraft.pck`. You can select it manually in the wizard
or set `WONDERDRAFT_PCK` to the complete path.

**Symbols are missing from an SVG export**

Open **Settings…** and check both asset paths. Custom assets normally come from
the folder configured in Wonderdraft's `config.ini`; built-in assets come from
the extracted `wonderdraft_files/sprites` folder.

**The file chooser or drag and drop does not work on Linux**

Start the app with `start_wonderdraft_editor_rust.sh`. The build and launcher
intentionally use X11 for compatible native file dialogs and drag and drop.

## Development and validation

```bash
cargo fmt -- --check
cargo test
cargo clippy --all-targets -- -D warnings
```

See [SVG_INTERCHANGE_NOTES.md](SVG_INTERCHANGE_NOTES.md) for format details and
[TEST_REPORT.md](TEST_REPORT.md) for the map/SVG verification scope.
