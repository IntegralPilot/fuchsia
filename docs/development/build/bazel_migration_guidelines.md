# Bazel migration guidelines

Last update: [2025-03-06](#document-history)

This page provides important Bazel-related guidelines for Fuchsia developers
working in-tree (i.e. with a fuchsia.git checkout). As the Bazel migration is
a moving target, this page will be updated frequently to reflect important
changes.

## Summary

As of Q1 2025, the following guidelines apply:

- If you do not perform product assembly, and do not write driver packages,
  *do not worry about or start writing Bazel files in the Fuchsia tree*.

- If you define new product boards or input bundles, do that with Bazel targets
  and dedicated Bazel SDK rules, as in
  [this example][example-bazel-board-definition]{:.external}.

[example-bazel-board-definition]: https://source.corp.google.com/h/fuchsia/fuchsia/+/main:boards/pkvm/BUILD.bazel

- If you write Bazel driver packages that are **only used by Bazel-defined
  boards**, then define them in Bazel only, using dedicated Bazel SDK rules, as
  in [this other example][bazel-driver-example]{:.external}.

[bazel-driver-example]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/simple/dfv2/BUILD.bazel;l=42

- All new board development should happen in Bazel. If your board happens
  to depend on an existing GN driver package, first expose it to the Bazel graph
  by setting the `export_to_bazel = true` argument in its GN
  [`fuchsia_driver_package()`][fuchsia_driver_package_export_to_bazel]{:.external},
  definition, then create a Bazel
  [`fuchsia_prebuilt_package`()][fuchsia_prebuilt_package]{:.external}
  companion target definition referencing the GN package artifact
  ([example here][fuchsia_driver_package_export_example]{:.external} and
  [here][fuchsia_prebuilt_package_example]{:.external}).

[fuchsia_driver_package_export_to_bazel]: https://cs.opensource.google/fuchsia/fuchsia/+/main:build/drivers/fuchsia_driver_package.gni;drc=82da562a4ef932e58704b61093bf80d564ac042f;l=42
[fuchsia_driver_package_export_example]: http://cs/h/fuchsia/fuchsia/+/main:src/connectivity/bluetooth/hci/transport/uart/cpp/BUILD.gn?l=79
[fuchsia_prebuilt_package]: https://cs.opensource.google/fuchsia/fuchsia/+/main:build/bazel_sdk/bazel_rules_fuchsia/fuchsia/private/fuchsia_prebuilt_package.bzl;drc=f48ae8ee65b68615d40b684fa9779fff7d413f44;l=326
[fuchsia_prebuilt_package_example]: http://cs/h/fuchsia/fuchsia/+/main:src/connectivity/bluetooth/hci/transport/uart/cpp/BUILD.bazel?l=12

- Bazel-based driver packages *must only* depend on platform libraries that are
  exposed through the `@fuchsia_sdk` and `@internal_sdk` repositories.

  `@fuchsia_sdk` contains [a set of libraries][fuchsia_idk.manifest]{:.external},
  that are distributed OOT, but does include "unstable" platform libraries to
  be used by driver developers (look for the `(unstable)` marker in lines of
  the previous link).

  For example [`@fuchsia_sdk//pkg:driver_power_cpp`][fuchsia_idk-driver_power_cpp]{:.external}
  exposes the library from the GN [`//sdk/lib/driver/power/cpp`][sdk-lib-driver-power-cpp]{:.external}
  library definition.

  `@internal_sdk` contains similar definitions, but only for SDK atoms that are
  only available within the in-tree Fuchsia build, and thus can be even more
  unstable. However [its removal is planned][internal_sdk-removal]{:.external},
  in favor of moving all unstable atoms to `@fuchsia_sdk` instead, so **no
  atoms should be added there**.

[fuchsia_idk.manifest]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/manifests/fuchsia_idk.manifest
[fuchsia_idk-driver_power_cpp]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/manifests/fuchsia_idk.manifest;drc=f094b00682c735aa7872a829749725ec3fdc9fa1;l=76
[sdk-lib-driver-power-cpp]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/power/cpp/BUILD.gn;drc=395158807df04cdff1efe9e59c296aaba3a8c8e8;l=18
[internal_sdk-removal]: https://fxbug.dev/333907192

- **Do not write BUILD.bazel files for internal platform libraries**.

  For now, duplicate BUILD.gn / BUILD.bazel definitions for the same non
  SDK libraries **should be avoided**.

  If your driver needs to use a platform-internal library that has not been
  exposed as an unstable IDK atom yet, contact
  [fuchsia-build-discuss@google.com][fuchsia-build-discuss]{:.external} to
  present your use case.

  Ideally, adding a new unstable IDK atom should be enough, but there may
  be exceptions, as described
  [in the dedicated section below](#exposing-platform-libraries-as-sdk-atoms).

- All Bazel targets must still be wrapped by a GN target to be visible
  to post-build clients.

  In particular, one or more Bazel *test packages* must be wrapped by a GN
  [`bazel_test_package_group()`][bazel_test_package_group]{:.external} target
  definition, to ensure that `fx test` and `botanist` (our infra CI test runner
  tool) can know about them, build them on demand, and execute them.

[bazel_test_package_group]: https://cs.opensource.google/fuchsia/fuchsia/+/main:build/bazel/bazel_test_package_group.gni;l=9

- Invoking Bazel from a GN/Ninja action is *very slow*: it takes several
  seconds *even if Bazel decides to do nothing*. Ninja batches them up, and runs
  all the waiting, runnable, Bazel actions as a group.

  These actions are defined by GN templates such as
  [`bazel_action`][bazel_action]{:.external}, or one of its wrappers,.

[bazel_action]: https://cs.opensource.google/fuchsia/fuchsia/+/main:build/bazel/bazel_action.gni;drc=d37d440b207387ed118b3165f3568f1691925aad;l=22
[bazel_test_package_group]: https://cs.opensource.google/fuchsia/fuchsia/+/main:build/bazel/bazel_test_package_group.gni;drc=00b94e4924d42da5ac5a3488c69b446973697316;l=9

- **Avoid non-terminal GN targets that depend on Bazel artifacts**

  Due to the high-cost of crossing the GN/Bazel boundary, dependency
  chains that look like `GN -> Bazel -> GN -> Bazel -> GN` result in
  significantly slower incremental builds, and must be avoided.

  See [dedicated section below](#dependencies-between-the-gn-and-bazel-graphs)
  for details.

## Dual `BUILD.bazel` and `BUILD.gn` definitions

It is inevitable that many of our targets will require dual definitions
in equivalent `BUILD.gn` and `BUILD.bazel` files during the rest of the
migration.

It is *critical* that such dual build files, if they exist, be kept in sync
over time. It is also important to track which dual-defined targets exist
in both graphs.

An initial and simple way to do that is to write them manually and use
matching `LINT` If-This-Then-That blocks in both files to detect skew
during CL review.

A better way, is to use a tool such as the recently-introduced
[`bazel2gn`][bazel2gn]{:.external} to automatically convert a `BUILD.bazel` to
an equivalent `BUILD.gn`.

[bazel2gn]: https://cs.opensource.google/fuchsia/fuchsia/+/main:build/tools/bazel2gn/BUILD.gn;drc=233558bd143f1084917d208561495f6364c09088;l=38

For now, `bazel2gn` is only a prototype, that only handles Go targets. It will
be improved over time to support more use cases, but it should not be used by
Fuchsia developers yet, its use being restricted to the Fuchsia Build Team.

In both cases, comment markers, and associated tooling, will be introduced to
track which targets are dual-defined in both graphs.

Note: Due to its declarative nature, and the availability of good Starlark
parsers, it is far simpler to convert from Bazel to GN, than the opposite.
Hence, there is no plan to support automatic translation from GN to Bazel.
Moreoever, certain GN/Ninja features like [`depfiles`][gn-depfiles]{:.external}
are impossible to express in the Bazel language.

[gn-depfiles]: https://gn.googlesource.com/gn/+/main/docs/reference.md#var_depfile


## Dependencies between the GN and Bazel graphs

Due to the high-cost of crossing the GN/Bazel boundary, dependency chains that
look like `GN -> Bazel -> GN -> Bazel -> GN` result in significantly slower
incremental builds Developer experience is also frustrating due to lack of
clarity when looking at real dependencies, and build failures can become much
harder to understand and fix.

At the moment, the minimum chain looks like `GN -> Bazel -> GN` because the
whole build is controlled by GN, and post-build clients only view GN-specific
target definitions, so any Bazel artifacts must be wrapped through GN-specific
targets.

The build team is working actively on making Bazel targets natively visible to
avoid that last `Bazel -> GN` edge.

This requires modifying post-build tools and scripts (e.g. `fx test` and many
others) to see Bazel outputs directly, and be able to rebuild them on demand
without invoking Ninja.


## Exposing platform libraries as SDK atoms

For a target to be visible in `@fuchsia_sdk`, the following must be true:

- Their type must match
  [one of our supported IDK atom schemas][idk-atom-schemas]{:.external}, which
  means:

  - No Rust source libraries.
  - No Go source libraries.

[idk-atom-schemas]: https://cs.opensource.google/fuchsia/fuchsia/+/main:build/sdk/meta/

- They must be defined in the GN graph (there is no support for defining
  SDK/IDK atoms in Bazel right now).

- They cannot be `testonly = true`, due to restrictions at the GN / Bazel
  boundary.

- They cannot have *conditional dependencies*. Our IDK schema does not support
  them, and for good reasons.

- Source libraries cannot depend on non-SDK libraries. *All their transitive
  dependencies must be part of the SDK too*.

- They must be defined with an SDK-compatible GN template, e.g.:

  - [`sdk_source_set()`][sdk_source_set]{:.external}: for C++ source libraries.
  - [`sdk_static_library()`][sdk_static_library]{:.external}: for prebuilt static C++ libraries.
  - [`sdk_shared_library()`][sdk_shared_library]{:.external}: for prebuilt shared C++ libraries.
  - [`fidl()`][fidl_template]{:.external} with `sdk_category` set: for FIDL definition files.
  - [`zx_library()`][zx_library]{:.external} with `sdk_publishable` set to an SDK category:
    for C++ source, static, and shared libraries that also need to be linked
    into the kernel, the bootloader(s) or the `userboot` program.
  - [`sdk_fuchsia_package()`][sdk_fuchsia_package]{:.external}: for prebuilt Fuchsia packages.

[sdk_source_set]: https://cs.opensource.google/fuchsia/fuchsia/+/main:build/cpp/sdk_source_set.gni
[sdk_static_library]: https://cs.opensource.google/fuchsia/fuchsia/+/main:build/cpp/sdk_static_library.gni
[sdk_shared_library]: https://cs.opensource.google/fuchsia/fuchsia/+/main:build/cpp/sdk_shared_library.gni
[fidl_template]: https://cs.opensource.google/fuchsia/fuchsia/+/main:build/fidl/fidl.gni
[zx_library]: https://cs.opensource.google/fuchsia/fuchsia/+/main:build/zircon/zx_library.gni
[sdk_fuchsia_package]: https://cs.opensource.google/fuchsia/fuchsia/+/main:build/packages/sdk_fuchsia_package.gni

If your platform library cannot follow these restrictions, contact the
[fuchsia-build-discuss@google.com][fuchsia-build-discuss]{:.external}
mailing list to discuss alternatives. Alternatives may include dual-graph
target definitions that need to be tracked for the rest of the migration.

[fuchsia-build-discuss]: mailto:fuchsia-build-discuss@google.com

## Document History

2025-03-06: Fix example on how to export Bazel package to GN.
2025-02-21: Initial version
