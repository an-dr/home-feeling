# Building and releasing

`cargo xtask package --platform <windows-x64|windows-arm64|linux-x64>` builds a release and writes the zip and its update manifest to `release/`. It builds for the machine it runs on unless given `--target <triple>`.

Work lands on `dev`; `main` holds released versions only. A release goes in three steps:

1. On `dev`, add the version's section to [CHANGELOG.md](../CHANGELOG.md), set the version in `[workspace.package]` of the root `Cargo.toml` and `UPGRADER_LAUNCHER_VERSION` in `.cargo/config.toml` to a release candidate such as `0.1.0-rc.1`, and push a matching tag. `cargo test` and `cargo xtask package` both fail when the two versions differ.
2. Try the pre-release. Another candidate bumps the number, `0.1.0-rc.2`.
3. On `dev`, set both versions to `0.1.0`, merge `dev` into `main` with a merge commit, and tag the merge commit.

```sh
git tag v0.1.0-rc.1 && git push origin dev v0.1.0-rc.1
# later, with 0.1.0 committed on dev:
git switch main && git merge --no-ff dev
git tag v0.1.0 && git push origin main v0.1.0
```

The [build workflow](../.github/workflows/build.yml) tests and packages on all three platforms, then publishes a GitHub release with the version's CHANGELOG section as its notes; a tag with a pre-release part publishes a pre-release. A tag that does not match the version fails the build. Installed apps never update to a pre-release. Installed apps look for updates in the latest release of `an-dr/home-feeling`, and the install scripts download from it, so the repository must be public for either to work.

Release builds for Linux run in an `ubuntu:22.04` container, so the binary runs on glibc 2.35 or newer whatever the CI runner ships. Do not move it to a newer image without raising the requirement in the README.

An update replaces the app in the install folder (`~/.home-feeling`), never the Things folder. A built-in Thing the user edited is left alone; its newer version is written beside it as `<file>.new`.
