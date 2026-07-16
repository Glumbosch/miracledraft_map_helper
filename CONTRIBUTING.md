# Contributing

Thanks for helping improve Wonderdraft Map Editor.

## Before opening an issue

- Search existing issues first.
- Reproduce the problem with the latest release when possible.
- Remove private paths and proprietary map or asset data from logs and samples.
- Never upload Wonderdraft's application files, extracted resources, or paid
  asset packs. Create the smallest synthetic map or fixture that demonstrates
  the problem.

## Development setup

Install Rust 1.87 or newer, clone the repository, and run:

```bash
cargo build --locked
cargo test --locked
```

Linux builds use X11. The launcher sets the expected toolkit backend:

```bash
./start_wonderdraft_editor_rust.sh
```

## Pull requests

Keep changes focused and describe how they were tested. Before submitting, run:

```bash
cargo fmt --all -- --check
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
```

Do not commit generated settings, extracted `wonderdraft_files`, maps, cache
data, or build output. New behavior should include tests where practical.

By contributing, you agree that your contribution is licensed under the MIT
License used by this repository.
