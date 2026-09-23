# ffx_e2e_emu

A Rust library for end-to-end testing of ffx tool and plugin behavior. Starts an
isolated emulator instance with the system image from the main build and allows
issuing ffx & ssh commands to it.

## Build setup

Define tests with the `ffx_e2e_test()` template in GN by importing
`//src/developer/ffx/lib/e2e_emu/ffx_e2e_test.gni`. See [existing users] of this
library for examples.

By convention tests written using this library are placed in a tree of
[`host_tests` groups][host_tests]. This name differentiates them from `tests`
and `e2e_tests` which typically run with a developer- or infra-managed Fuchsia
device, whereas these tests start their own emulator instance.

Add any Fuchsia package dependencies required by your test to
`//src/developer/ffx:package_deps_for_host_tests`,
usually by adding similarly-named groups to the build for your test and the
BUILD.gn files in any parent directories.

[existing users]: https://cs.opensource.google/search?q=ffx_e2e_test%5C(%20file:BUILD.gn&sq=&ss=fuchsia
[host_tests]: https://cs.opensource.google/search?q=file:BUILD.gn%20group%5C(%5C%22host_tests%5C%22%5C)&ss=fuchsia

## Customizing system image

If your test requires any Fuchsia features, packages, or other dependencies
beyond the sparse default image provided, you can use the
`ffx_e2e_product_bundle()` template to define your own product bundle.

See existing [users of custom product bundles][custom_pbs] for examples.

[custom_pbs]: https://cs.opensource.google/search?q=file:BUILD.gn%20ffx_e2e_test%20%22product_bundle%20%3D%20%22&ss=fuchsia

## Serving packages

If your test needs to serve custom packages from a repository server instead of
including them in the system image, you can define a package repository with
`ffx_e2e_package_repository()` and it will be connected to your emulator when
the test starts.

See existing [users of custom package repos][custom_repos] for examples.

[custom_repos]: https://cs.opensource.google/search?q=file:BUILD.gn%20ffx_e2e_test%20%22package_repository%20%3D%20%22&sq=&ss=fuchsia

## Caveat: Isolated emulator tests are slow

This is because of 2 reasons:

1. spinning up a full Fuchsia instance for a test can take 20+ seconds
2. only one ffx isolate can be in use at once, serializing test execution
