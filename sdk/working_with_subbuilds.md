# Working with IDK Sub-builds

## Sub-build output directory

The output directory for a sub-build is a subdirectory under the main build
output directory. For example,
`out/default/idk_subbuild.fuchsia_collection_for_subbuilds-apiNEXT-x64/`.

## Building all sub-builds {#building-all-sub-builds}

All sub-builds are built as part of the IDK build:
```
fx build //sdk:final_fuchsia_idk
```

The following is faster because it avoids the archive generation step:
```
fx build //sdk:final_fuchsia_idk.validation
```

## Building a subset of sub-builds

### Sub-builds for a subset of target CPU architectures

The build time for this can be cut by about two thirds by adding the following
to your `fx args` (when the target CPU architectures is `x64`):
```
idk_buildable_cpus = [ "x64" ]
```

### Sub-builds for a subset of API levels

Similarly, you may wish to build a set of API levels for coverage without having
to build every API level. In such cases, you can add something like the
following to your `fx args`:
```
idk_buildable_api_levels = [ 16, 22, "NEXT" ]
```

## Building a specific sub-build

### Setup

**Before working with sub-builds directly, you must have generated the sub-build
directories by generating build files via the main build.**
The quickest way to do this is to build the sub-build once as follows:
```
fx build //sdk:fuchsia_collection_subbuilds-api${level}-${arch}
```

For example:
```
fx build //sdk:fuchsia_collection_subbuilds-apiNEXT-x64
```

### Building

You can directly build a sub-build with the following command. This can be
useful when debugging a build failure at a specific API level.
```
fx build -- -C $(fx get-build-dir)/idk_subbuild.fuchsia_collection_for_subbuilds-api${level}-${arch}
```

You can also build specific Ninja targets as in the following example for fdio:
```
fx build -- -C $(fx get-build-dir)/idk_subbuild.fuchsia_collection_for_subbuilds-api${level}-${arch} sdk/lib/fdio
```

Building specific Ninja targets in another toolchain requires identifying the
correct path, as in the following example for the FIDL library `fuchsia.images2`:
```
fx build -- -C $(fx get-build-dir)/idk_subbuild.fuchsia_collection_for_subbuilds-api${level}-${arch} fidling/phony/sdk/fidl/fuchsia.images2/fuchsia.images2
```

#### Example: Building the `NEXT-x64` sub-build

The following are specific examples of the above commands for the `x64` at
`NEXT` sub-build.

Build the entire `NEXT` sub-build as follows:
```
fx build -- -C $(fx get-build-dir)/idk_subbuild.fuchsia_collection_for_subbuilds-apiNEXT-x64
```

Build specific Ninja targets, such as fdio:
```
fx build -- -C $(fx get-build-dir)/idk_subbuild.fuchsia_collection_for_subbuilds-apiNEXT-x64 sdk/lib/fdio
```

Building specific Ninja targets in another toolchain, such the FIDL library
`fuchsia.images2`:
```
fx build -- -C $(fx get-build-dir)/idk_subbuild.fuchsia_collection_for_subbuilds-apiNEXT-x64 fidling/phony/sdk/fidl/fuchsia.images2/fuchsia.images2
```

### Using GN

To use GN commands, you must always specify the following:

* `--root-pattern=//:build_only`
  * Explanation: The `args.gn` in the sub-build output directory specifies
    `build_only_labels = ["//sdk:fuchsia_collection_for_subbuilds"]`.
* The sub-build output directory in which to perform the GN commands.
  * For example,
    `$(fx get-build-dir)/idk_subbuild.fuchsia_collection_for_subbuilds-apiNEXT-x64/`.

#### Example: Using GN on the `NEXT-x64` sub-build

You can regenerate GN targets:
```
fx gn --root-pattern=//:build_only gen $(fx get-build-dir)/idk_subbuild.fuchsia_collection_for_subbuilds-apiNEXT-x64/
```

You can also perform GN analysis on the sub-build, such as to find all paths
from the sub-build root (`//sdk:fuchsia_collection_for_subbuilds`) to fdio:
```
fx gn --root-pattern=//:build_only path --with-data --all $(fx get-build-dir)/idk_subbuild.fuchsia_collection_for_subbuilds-apiNEXT-x64/ //sdk:fuchsia_collection_for_subbuilds //sdk/lib/fdio
```

Or to find reverse dependencies:
```
fx gn --root-pattern=//:build_only refs $(fx get-build-dir)/idk_subbuild.fuchsia_collection_for_subbuilds-apiNEXT-x64 $(fx get-build-dir)/idk_subbuild.fuchsia_collection_for_subbuilds-apiNEXT-x64/gen/sdk/lib/fdio/fdio.verify_public_headers.stamp
```

To clean just the sub-build, run:
```
fx gn clean $(fx get-build-dir)/idk_subbuild.fuchsia_collection_for_subbuilds-apiNEXT-x64
```

