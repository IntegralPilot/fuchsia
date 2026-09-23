# package-directory compatibility tests

Integration tests for
[package-directory](https://cs.opensource.google/fuchsia/fuchsia/+/main:src/sys/pkg/lib/package-directory/).

To run this test suite against a package directory backed by a different implementation, you have to create
a new puppet component to serve that implementation. Follow these steps:

1. Create a new `./{name}-puppet` directory (example: `./pkgdir-puppet`).
2. Implement a new puppet component (see the source code from the previous example).
3. Create a new `fuchsia_package("{name}-integration-tests") { .. }` target with the new puppet as a subpackage. `meta/test-root.cml` will look for the puppet component at the subpackage URL `puppet#meta/default.cm`