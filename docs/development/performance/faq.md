# FAQ

## Why does my performance test get run twice in Infra builds?

For performance tests that consist of a `fuchsia_test_package` and a
host-side wrapper, the test logic can end up getting run twice in
Infra builds: once through the host-side wrapper, and once without the
wrapper as a Fuchsia-side-only test.

That is undesirable because: 1) it wastes Infra capacity, and 2) the
test package sometimes ends up getting run in unusual or slow build
configurations (e.g. ASan, coverage, QEMU) where it is flaky or takes
far too long to run.

(However, sometimes it can be desirable, such as when the test package
runs its tests in "unit test mode" by default, i.e. with a small
number of test iterations, rather than "performance test mode".)

The problem arises because adding a `fuchsia_test_package` to the set
of dependencies for the configuration for a builder generally causes
the builder to run it as a Fuchsia-side-only (target-side) test.

The recommended fix for this problem is to add the following to the
`fuchsia_test_package` declaration in the `BUILD.gn` file:

```gn
  # Prevent this test from being run as a target-side test, because it
  # is run by a host-side wrapper.
  test_specs = {
    environments = []
  }
```
