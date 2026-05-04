
# CI Matrix Expansion Note

The CI workflow should be expanded to include:
- macOS-13 (x86_64) for backward compatibility
- MSRV (Minimum Supported Rust Version) verification: 1.80
- Windows and Linux runners for cross-platform validation

Add to .github/workflows/ci.yml strategy.matrix:
```yaml
os:
  - macos-latest
  - macos-13
  - ubuntu-latest
rust:
  - stable
  - 1.80  # MSRV
```
