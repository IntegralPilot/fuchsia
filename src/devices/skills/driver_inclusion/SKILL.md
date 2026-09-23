---
name: driver-inclusion
description: >
  Add a driver to a Bazel-built boot image for a specific hardware target/board.
  Use when a driver was added via GN args or `fx add-test` but is still missing
  from `ffx driver list` on a Bazel-based board, or when bringing up a new board
  target. Requires declaring fuchsia_cc_driver / fuchsia_driver_component /
  fuchsia_package in BUILD.bazel and listing the package in the board's image
  definition (e.g. //vendor/google/boards/<target>/BUILD.bazel). Don't use for
  host/test-realm unit tests, where GN and `fx add-test` still apply.
---

# Driver Inclusion for Specific Hardware Targets

For certain hardware targets, the boot image is constructed using the Bazel
build system, typically defined in directories like `<repo>/boards/<target>/`.

## Driver Inclusion

To include a driver in the image for these targets, adding it via GN build
arguments (or its test via `fx add-test`) is not sufficient. You must define the
driver and its component using Bazel rules (e.g., `fuchsia_cc_driver`,
`fuchsia_driver_component` in a `BUILD.bazel` file) and ensure that package
target is listed in the board's image definition file (e.g.,
`//vendor/google/boards/<target>/BUILD.bazel`).

## Tests

If you are just adding unit tests that run on the host or in the test realm
without needing to be part of the boot image drivers, the `fx add-test` and GN
approach may still apply.
