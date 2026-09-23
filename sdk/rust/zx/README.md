# zx crate

Wraps the Zircon vDSO API for Rust, using a misuse-resistant design (like
`std::fs::File`) for kernel handles.

## Contributing and API Evolution

This crate is widely used which means that many targets can break due to changes
and incremental builds cover many thousands of targets.

To iterate quickly, use `fx clippy --all` instead of `fx build`.

To ensure your changes don't break non-default targets, use a maximal build
graph including buildbot targets. For example:

```sh
fx set core.x64 --debug \
    --with-host //bundles/buildbot/host \
    --with //bundles/buildbot/core \
    --with //vendor/google/bundles/buildbot/core
```

Note: Presubmit might still fail on other architectures (arm64, riscv64) or
specific tests. Check infra logs and builder config if that happens.

Depending on the breadth of your change's impact, it may be worthwhile to
expand the list of buildbot bundles. Take a look at the subdirectories of
`//bundles/buildbot` and `//vendor/google/bundles/buildbot` for options.
