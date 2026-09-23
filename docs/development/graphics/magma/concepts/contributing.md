# Contributing to Magma and best practices

## Submitting a patch

See [Contributing](/CONTRIBUTING.md).

## Source code

The core magma code is found under:

* [lib/magma](/src/graphics/lib/magma)

Magma service drivers are found under:

* [src/graphics/drivers](/src/graphics/drivers)

Magma client drivers are third party codebases.  Open source client drivers
are in [third_party/mesa](https://fuchsia.googlesource.com/third_party/mesa).

## Coding conventions and formatting

* Use the
  **[Google style guide](https://google.github.io/styleguide/cppguide.html)**
  for source code.
* Run **clang-format** on your changes to maintain consistent formatting.

## Build Configuration for Testing

### Product for L0 testing

* `core`

### Packages for L0 testing

* `src/graphics/lib/magma/tests:l0`

### Product for L1 testing

* `workbench_eng`

### Package for L1 testing

* `src/graphics/examples:vkcube-on-scenic`

## Testing Pre-Submit

For details on the testing strategy for magma, see [Test Strategy](test_strategy.md).

There are multiple levels for magma TPS. Each level includes all previous
levels.

When submitting a change, indicate the TPS level tested, prefaced by the
hardware on which you performed testing:

TEST:
nuc,vim3:go/magma-tps#L1
nuc,vim3:go/magma-tps#S1
nuc,vim3:go/magma-tps#C0
nuc,vim3:go/magma-tps#P0

### L0

1. Build Fuchsia:

   ```posix-terminal
   fx build
   ```

2. Run the test script
   [src/graphics/lib/magma/scripts/test.sh](/src/graphics/lib/magma/scripts/test.sh):

  ```posix-terminal
  ./src/graphics/lib/magma/scripts/test.sh
  ```

### L1

If you have an attached display, execute the spinning
[vkcube](/src/graphics/examples/vkcube). This test uses an imagepipe swapchain
to pass frames to the system compositor. Build with
`--with-test src/graphics/examples:vkcube-on-scenic`.

Test with present through Scenic:

```posix-terminal
ffx session add fuchsia-pkg://fuchsia.com/vkcube-on-scenic#meta/vkcube-on-scenic.cm`
```

### S0

Run vkcube-on-scenic overnight (12-24 hours).

### S1

A full UI stress test. Launch two instances of the `spinning_cube` flutter
 example and let them run overnight.

### C0

For some changes, it's appropriate to run the Vulkan conformance test suite
before submitting. See [Conformance](#conformance).

### P0

For some changes, it's appropriate to run benchmarks to validate performance
metrics. See [Benchmarking](#benchmarking).

## Conformance

For details on the Vulkan conformance test suite, see:

* [third_party/vulkan-cts](https://fuchsia.googlesource.com/third_party/vulkan-cts/+/HEAD/README.md)

## See also

* [Test Strategy](test_strategy.md)
