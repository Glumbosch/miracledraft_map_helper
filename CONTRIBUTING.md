# Contributing

Thanks for wanting to help with this project. I don't inted to maintain this project. As this is a small one time use tool for me.

## Development setup

Install Rust 1.87 or newer, clone the repository, and run:

```bash
cargo build --locked
cargo test --locked
```

Linux builds use X11. The launcher sets the expected toolkit backend:

```bash
./start_miracledraft_map_helper.sh
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

By contributing, you agree to dedicate any copyright and related rights you
hold in the contribution to the public domain under the repository's
[Unlicense](LICENSE), to the fullest extent permitted by law.
