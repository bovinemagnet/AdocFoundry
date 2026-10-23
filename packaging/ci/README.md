# CI and release workflows (not active)

These GitHub Actions workflows are kept here, outside `.github/workflows/`, so GitHub does not run them yet. macOS and Windows are being verified by hand first.

- `ci.yml`: fmt, clippy, `cargo deny`, `cargo xtask layers`, tests on Ubuntu, macOS and Windows, and the grammar corpus tests.
- `release.yml`: on a `v*` tag, the macOS DMG, Windows MSI and zip, and Linux AppImage, deb and rpm, attached to a GitHub release.

To enable them, move both files back into `.github/workflows/`.
