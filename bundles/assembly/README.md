# Assembly Input Bundles (AIBs) and the GN / Bazel Bridge

## Overview

Platform Assembly Input Bundles (AIBs) are **now** defined natively in Bazel
within `//bundles/assembly/BUILD.bazel`. However, during the transition from
GN to Bazel, many artifacts (Fuchsia packages, compiled config files, etc.) are
still built by GN.

To bridge this gap, `//bundles/assembly/bazel_inputs` provides a structured
mechanism for getting GN-built artifacts into the Bazel AIB definitions. For
automated assistance when syncing Bazel targets to GN targets, reference the
`update-assembly-bundles` skill
(`//build/assembly/skills/update-assembly-bundles/SKILL.md`).

## Architecture & Directory Structure

The bridging mechanism relies on a parallel directory structure in
`//bundles/assembly/bazel_inputs/...` that mirrors the true GN target
locations:

*   `//bundles/assembly/BUILD.bazel`: Contains the true
    `assembly_input_bundle()` target definitions.
*   `//bundles/assembly/bazel_inputs/BUILD.gn`: Contains GN `group()` targets
    named after each AIB. These groups collect all the `bazel_input_*()` and
    `export_package_to_bazel()` targets that provide GN/Ninja-built artifacts
    needed by the Bazel AIB definitions.
*   `//bundles/assembly/bazel_inputs/<gn_path>/BUILD.gn`: Defines how a GN
    target from `//<gn_path>` is exported to Bazel.
*   `//bundles/assembly/bazel_inputs/<gn_path>/BUILD.bazel`: (For packages
    only) Imports the exported package into Bazel as a `prebuilt_package()`.

> [!IMPORTANT]
> Pure source files (e.g. static `.cml` files in source control) do **not** use
> `bazel_inputs`. They are referenced directly in `BUILD.bazel` (e.g.
> `//src/sys/root:root.cml`) provided their local `BUILD.bazel` exports them
> via `exports_files()`. `bazel_inputs` is strictly for GN **build targets**
> (packages and generated files).

---

## Common Workflows

When modifying AIBs that require adding, removing, or updating GN-produced
inputs, follow these workflows. We will use the `common_standard` AIB as our
running example.

### 1. Adding a New Package from GN

To add a GN package `//src/cobalt/bin/app:cobalt` to the `common_standard` AIB:

1.  **Export from GN:** Create
    `//bundles/assembly/bazel_inputs/src/cobalt/bin/app/BUILD.gn`:
    ```gn
    import("//build/bazel/export_fuchsia_package_to_bazel.gni")

    export_fuchsia_package_to_bazel("cobalt") {
      package = "//src/cobalt/bin/app:cobalt"
    }
    ```
2.  **Import to Bazel:** Create
    `//bundles/assembly/bazel_inputs/src/cobalt/bin/app/BUILD.bazel`:
    ```python
    load(
        "//build/bazel/rules/packages:prebuilt_package.bzl",
        "prebuilt_package",
    )

    package(default_visibility = ["//bundles/assembly:__subpackages__"])

    prebuilt_package(
        name = "cobalt",
        archive = "@gn_targets//bundles/assembly/bazel_inputs/" +
                  "src/cobalt/bin/app:cobalt",
    )
    ```
3.  **Register in GN AIB Group:** In
    `//bundles/assembly/bazel_inputs/BUILD.gn`, locate the group
    corresponding to the AIB (`group("common_standard")`) and add the export
    target to its `deps`:
    ```gn
    group("common_standard") {
      deps = [
        ...
        "//bundles/assembly/bazel_inputs/src/cobalt/bin/app:cobalt",
      ]
    }
    ```
4.  **Include in Bazel AIB:** In `//bundles/assembly/BUILD.bazel`, add the
    imported package label to the appropriate list (e.g., `base_packages`) of
    the `assembly_input_bundle()` target:
    ```python
    assembly_input_bundle(
        name = "common_standard",
        ...
        base_packages = [
            ...
            "//bundles/assembly/bazel_inputs/src/cobalt/bin/app:cobalt",
        ],
    )
    ```

### 2. Deleting a Package from GN

To remove a GN package (`//src/cobalt/bin/app:cobalt`) from an AIB:

1.  Remove the reference
    (`"//bundles/assembly/bazel_inputs/src/cobalt/bin/app:cobalt"`) from the
    `assembly_input_bundle()` target in `//bundles/assembly/BUILD.bazel`.
2.  Remove the reference from the corresponding AIB group in
    `//bundles/assembly/bazel_inputs/BUILD.gn`.
3.  If no other AIB groups in `bazel_inputs/BUILD.gn` depend on this package,
    delete the `BUILD.gn` and `BUILD.bazel` files in
    `//bundles/assembly/bazel_inputs/src/cobalt/bin/app` (and delete the
    directory if empty).

### 3. Adding a New Single File Target from GN

For GN targets that output a single file (e.g. compiled config JSONs,
protobufs) rather than a full package:

1.  **Export from GN:** Create (or edit)
    `//bundles/assembly/bazel_inputs/src/cobalt/bin/app/BUILD.gn`:
    ```gn
    import("//build/bazel/bazel_inputs.gni")

    bazel_input_file("global_metrics_registry_pb") {
      generator = "//src/cobalt/bin/app:global_metrics_registry"
      outputs = [
        "$root_gen_dir/src/cobalt/bin/app/global_metrics_registry.pb",
      ]
      # Explicit label for @gn_targets
      gn_targets_name = "global_metrics_registry_pb"
    }
    ```
2.  **Register in GN Group:** In `//bundles/assembly/bazel_inputs/BUILD.gn`,
    add the target to the specific AIB group (`group("common_standard")`):
    ```gn
    group("common_standard") {
      deps = [
        ...
        # Reference local target or subdir target
        ":global_metrics_registry_pb",
      ]
    }
    ```
3.  **Reference Directly in Bazel:** Do **not** create a `BUILD.bazel` file in
    `bazel_inputs/...` for single files. Instead, reference the generated
    `@gn_targets` label directly in the AIB definition in
    `//bundles/assembly/BUILD.bazel`.

    ```python
    assembly_input_bundle(
        name = "common_standard",
        ...
        config_data = [
            {
                "package_name": "cobalt",
                "files": [
                    {
                        "source": "@gn_targets//src/cobalt/bin/app:" +
                                  "global_metrics_registry_pb",
                        "destination": "global_metrics_registry.pb",
                    },
                ],
            },
        ],
    )
    ```

### 4. Deleting a Single File Target from GN

1.  Remove the `@gn_targets//src/cobalt/bin/app:global_metrics_registry_pb`
    reference from the AIB in `//bundles/assembly/BUILD.bazel`.
2.  Remove the reference from `//bundles/assembly/bazel_inputs/BUILD.gn`.
3.  Remove the `bazel_input_file()` definition from its GN file. Delete the
    `BUILD.gn` file and directory if no other targets remain.

### 5. Renaming a Package Target from GN

If a GN package target is renamed (e.g., `//src/cobalt/bin/app:old` ->
`//src/cobalt/bin/app:cobalt`):

1.  Update the `package` parameter inside `export_fuchsia_package_to_bazel` in
    `//bundles/assembly/bazel_inputs/src/cobalt/bin/app/BUILD.gn`.
2.  If the directory path changed (e.g., `//src/old` -> `//src/new`), move the
    `bazel_inputs` directory to match (`bazel_inputs/src/new`). Update all
    references in `bazel_inputs/BUILD.gn` and `bundles/assembly/BUILD.bazel`
    to the new path.

### 6. Renaming a Single File Target from GN

If the underlying GN generator target is renamed
(`//src/cobalt/bin/app:old_gen` ->
`//src/cobalt/bin/app:global_metrics_registry`):

1.  Update `generator`, `outputs`, and `gn_targets_name` in
    `bazel_input_file()` within
    `//bundles/assembly/bazel_inputs/src/cobalt/bin/app/BUILD.gn`.
2.  Update the `@gn_targets` label in `//bundles/assembly/BUILD.bazel` to match
    the new `gn_targets_name`
    (`@gn_targets//src/cobalt/bin/app:global_metrics_registry_pb`).

### 7. Updating AIBs After Full Migration of an Input to Bazel

When an artifact (package or file) has been fully migrated to build natively
in Bazel:

1.  **Remove GN Bridge Scaffolding:**
    *   Remove the dependency entry from
        `//bundles/assembly/bazel_inputs/BUILD.gn`.
    *   Delete the `BUILD.gn` and `BUILD.bazel` files in the bridging directory
        (`//bundles/assembly/bazel_inputs/src/cobalt/bin/app`), and delete the
        directory if empty.
2.  **Update Bazel AIB Definition:** In `//bundles/assembly/BUILD.bazel`,
    replace the bridge label
    (`//bundles/assembly/bazel_inputs/src/cobalt/bin/app:cobalt`) or
    `@gn_targets` label with the true native Bazel target label
    (`//src/cobalt/bin/app:cobalt`).

