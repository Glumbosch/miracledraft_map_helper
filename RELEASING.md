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
   git tag -a v0.4.5 -m "Release v0.4.5"
   git push origin main v0.4.5
   ```

5. Watch the **Release** workflow. It validates that `v0.4.5` matches the Cargo
   package version, builds each platform archive, publishes checksums and build
   provenance, creates the GitHub release, and publishes the Linux, Windows,
   and macOS builds to [itch.io](https://glumbosch.itch.io/miracledraft-map-helper).
6. Download an archive from the published release and perform a smoke test.

The itch.io publishing steps use the `buttler_api_key` GitHub Actions secret.
The Linux, Windows, and macOS builds are published to the `linux`, `windows`,
and `osx` channels respectively. The itch.io project slug is
`glumbosch/miracledraft-map-helper`.

If a release workflow fails, fix the cause and create a new patch version. Do
not silently move a tag after users may have downloaded artifacts.
