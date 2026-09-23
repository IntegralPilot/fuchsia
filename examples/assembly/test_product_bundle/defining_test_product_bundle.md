# Defining a test product bundle

[go/multi-product-builds](http://goto.google.com/multi-product-builds) allows
Fuchsia developers to construct a custom product bundle that tests their code
without needing to define a new builder. This document describes the steps to
accomplish this.

## 1. Declare a new product bundle

This can be done in either GN or Bazel.
While we are actively moving most build actions to Bazel, it is currently
simpler and faster to write this in GN.

### Product Bundle in GN

Declare your product bundle in GN.

```gn
# //examples/assembly/test_product_bundle/BUILD.gn
import("//build/assembly/test_product_bundle.gni")
import("//boards/default_for_emulator.gni")

test_product_bundle("test_product.arm64") {
  board_config = default_board_for_emulator
  platform = {
    build_type = "eng"
  }
}
```

### Product Bundle in Bazel

Declare your product bundle in Bazel.

NOTE: We do not currently recommend this approach because it will make the
build slower.

```starlark
# //examples/assembly/test_product_bundle/BUILD.bazel
load(
    "@rules_fuchsia//fuchsia:assembly.bzl",
    "fuchsia_test_product_bundle",
)

fuchsia_test_product_bundle(
    name = "test_product.arm64",
    board_config = "//boards:arm64",
    product_config_json = {
        "platform": {
            "build_type": "eng",
        },
    },
    # Support running on an emulator.
    virtual_devices = [ "//build/bazel/assembly/virtual_devices:arm64-emu-recommended" ],
)
```

Declare a "bazel bridge" which hops from GN-land over to Bazel-land.

```gn
# //examples/assembly/test_product_bundle/BUILD.gn
import("//build/bazel/assembly/bazel_test_product_bundle.gni")

bazel_test_product_bundle("test_product.arm64") {
  # This is the name of the Bazel target.
  bazel_target = ":test_product.arm64"
  # These are additional GN dependencies to build and make available to Bazel.
  bazel_inputs_from_gn = [ "//boards/arm64:arm64.bazel_input" ]
}
```

## 2. Declare which tests should run on your product bundle

You need to tell the build system which tests should run on your new product bundle.
We use `product_bundle_test_group` for this.

```gn
# //examples/assembly/test_product_bundle/BUILD.gn
import("//build/assembly/product_bundle_test_group.gni")
import("//build/testing/environments.gni")

product_bundle_test_group("test_product_bundle") {
  product_bundle = ":test_product.arm64"
  environments = [ qemu_env ]
  target_tests = [ "//examples/testing/unittests/rust:reverser_rust_unittest" ]
  host_tests = []
}
```

## 3. Test manually

Add the test to your GN arguments:

```gn
developer_test_labels = [ "//examples/assembly/test_product_bundle" ]
```

Build the test and Product Bundle:

```sh
fx build
```

Find the Product Bundle in your out directory:

```sh
PB_RELATIVE=$(fx list-build-artifacts --name test_product.arm64 product-bundle)
PB=$(fx get-build-dir)/$PB_RELATIVE
```

Run the Product Bundle on a target:
```sh
ffx emu start $PB
fx set-device fuchsia-emulator
```

Run the test:
```sh
fx test reverser
```

## 4. Run in an infra builder

Adding the test to a builder is usually done by adding it to one of the groups
in `//bundles/buildbot`, then ensuring your builder includes that group.

Here is a sample builder:

```starlark
builders.fuchsia(
    "fuchsia.arm64-release",
    builder_groups = BLOCKING_GROUPS,
    owner = teams.SOFTWARE_ASSEMBLY,
    issue = "https://fxbug.dev/407825733",
    spec = fuchsia_spec.fuchsia(
        board = build.boards.arm64,
        compilation_mode = build.compilation_mode.release,
        include_images = False,
    ),
)
```

Adding a `//bundles/buildbot` group that includes both host and target tests
can be done as following:

```starlark
builders.fuchsia(
    "fuchsia.arm64-release",
    builder_groups = BLOCKING_GROUPS,
    owner = teams.SOFTWARE_ASSEMBLY,
    issue = "https://fxbug.dev/407825733",
    spec = fuchsia_spec.fuchsia(
        board = build.boards.arm64,
        compilation_mode = build.compilation_mode.release,
        include_images = False,

        # Groups of host tests.
        host_labels = [
            "//bundles/buildbot/my_subsystem",
        ],

        # Groups of target tests.
        universe_packages = [
            "//bundles/buildbot/my_subsystem",
        ],
    ),
)
```

Host tests in this situation are typically e2e tests that are driven by a host
script, while target tests are packages that run on a target.

`//bundles/buildbot/my_subsystem` would then include a dependency to your
`product_bundle_test_group()`:

```gn
# //bundles/buildbot/my_subsytem/BUILD.gn
group("my_subsystem") {
  testonly = true
  deps = [ "//examples/assembly/test_product_bundle" ]
}
```

We prefer that all infra builder only depend on groups in `//bundles/buildbot`
so that it is easy to understand what groups require soft transitions
with infrastructure and what can be treated as implementation detail. It
also allows us to reorganize our code without needing to update all the infra
builder configs.
