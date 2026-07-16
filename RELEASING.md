# Release checklist

1. Make sure `main` is green and the working tree contains only intentional
   release changes.
2. Update `version` in `Cargo.toml` and run `cargo check --locked`. Commit the
   resulting `Cargo.lock` change if Cargo updates it.
3. Run the local checks:

   ```bash
   cargo fmt --all -- --check
   cargo test --locked --all-targets
   cargo clippy --locked --all-targets -- -D warnings
   ```

4. Commit the release version, then create and push a matching tag:

   ```bash
   git tag -a v0.4.2 -m "Release v0.4.2"
   git push origin main v0.4.2
   ```

5. Watch the **Release** workflow. It validates that `v0.4.2` matches the Cargo
   package version, builds each platform archive, publishes checksums and build
   provenance, and creates the GitHub release.
6. Download an archive from the published release and perform a smoke test.

If a release workflow fails, fix the cause and create a new patch version. Do
not silently move a tag after users may have downloaded artifacts.
