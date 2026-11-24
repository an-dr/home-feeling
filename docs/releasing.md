# Building and releasing

`cargo xtask package --platform <windows-x64|windows-arm64|linux-x64>` builds a release and writes the zip and its update manifest to `release/`. It builds for the machine it runs on unless given `--target <triple>`.

To publish, set the version in `[workspace.package]` of the root `Cargo.toml` and `UPGRADER_LAUNCHER_VERSION` in `.cargo/config.toml` to the same value, get that commit onto `main`, and push a matching tag. `cargo test` and `cargo xtask package` both fail when the two versions differ.

```sh
git tag v0.1.0
git push origin main v0.1.0
```

The [build workflow](../.github/workflows/build.yml) tests and packages on all three platforms, then publishes a GitHub release. A tag that does not match the version fails the build. Installed apps look for updates in the latest release of `an-dr/home-feeling`, and the install scripts download from it, so the repository must be public for either to work.

Release builds for Linux run in an `ubuntu:22.04` container, so the binary runs on glibc 2.35 or newer whatever the CI runner ships. Do not move it to a newer image without raising the requirement in the README.

An update replaces the app in the install folder (`~/.home-feeling`), never the Things folder. A built-in Thing the user edited is left alone; its newer version is written beside it as `<file>.new`.
