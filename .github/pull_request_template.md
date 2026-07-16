## Summary

Describe the change and why it is needed.

## Validation

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo test --locked --all-targets`
- [ ] `cargo clippy --locked --all-targets -- -D warnings`
- [ ] Relevant manual workflow tested, if applicable

## Publication safety

- [ ] No generated config, maps, extracted Wonderdraft files, proprietary assets, caches, or build output are included
- [ ] User-facing behavior and documentation are updated where needed
