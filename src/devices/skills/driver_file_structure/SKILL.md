---
name: driver-file-structure
description: >
  Create or update a Fuchsia driver's directory layout, file names, and build
  target conventions. Covers the meta/ dir (.bind, .cml, -info.json), C++
  (.cc/.h) vs Rust (mandatory src/lib.rs) sources, and the four-target GN/Bazel
  chain (bind -> driver -> component -> package). Use when scaffolding a new
  driver or when build/bind targets fail to resolve due to hyphen-vs-underscore
  naming mismatches with the parent directory, or a missing dependency on the
  bind target.
---

# Driver File Structure

## Organize the Directory Layout

A typical driver directory in Fuchsia (e.g., `src/devices/.../my-driver/`)
follows this layout:

* **`meta/`**: Contains component manifests and metadata.
* **Source Files**: The implementation files (C++ or Rust).
* **Build Files**: Define how the driver is compiled and packaged.
* **Tests**: Unit and integration tests for the driver.

### Set Up the `meta/` Directory

The `meta/` directory contains files that define the driver's identity,
capabilities, and binding rules.

* **`[driver_name].bind`**: The source file containing the bind rules.
* **`[driver_name].cml`**: The component manifest defining capabilities and
  location in the topology.
* **`[driver_name]-info.json`**: Metadata about the driver (name, description,
  etc.).
* **`bind-tests.json`**: Defines tests for the bind rules (optional).

### Create the Source Files

Implementation files are located at the root of the driver directory or in
subdirectories if complex.

#### C++ Drivers

C++ drivers typically consist of the following file structure:
* **`[driver_name].cc`**: Contains the driver implementation, including the
  lifecycle hooks and the `FUCHSIA_DRIVER_EXPORT2` macro.
* **`[driver_name].h`**: Defines the driver class, inheriting from
  `fdf::DriverBase2`.

#### Rust Drivers

Rust drivers have a strict file structure enforced by the Rust compilation
model:
* **`src/lib.rs`**: The root library file. Fuchsia Rust drivers are compiled as
  dynamic libraries loaded by the driver host, so they **must** use `lib.rs`.
* **Other source files**: Additional modules can be included in the `src/`
  directory (e.g., `src/device.rs`, `src/protocol.rs`).
* **`src/main.rs`**: Used only for tools or executables that live within the
  driver directory, not for the driver itself.

### Configure the Build Files

Build files define the targets for compiling the driver, wrapping it in a
component, and creating the driver package.

* **`BUILD.gn`**: Used for the GN build system (in-tree).
* **`BUILD.bazel`**: Used for the Bazel build system (SDK and portable builds).

#### Standard GN Target Structure

A complete driver `BUILD.gn` should contain the following hierarchical targets
(using hyphens/underscores based on parent folder styling):

1.  **`driver_bind_rules`**: Generates bind code. Naming convention:
    `[driver-name]-bind`.
2.  **`fuchsia_rust_driver` / `fuchsia_cc_driver`**: Compiles the shared library
    binary. Naming convention: `[driver-name]-driver`.
3.  **`fuchsia_driver_component`**: Packages the manifest and binary together.
    Naming convention: `[driver-name]-component`.
4.  **`fuchsia_driver_package`**: The final deliverable package. Naming
    convention must match the parent directory exactly: `[driver-name]`.

```gn
import("//build/bind/bind.gni")
import("//build/drivers.gni")

# 1. Bind Rules (Same for both C++ and Rust)
driver_bind_rules("my-driver-bind") {
  rules = "meta/my-driver.bind"
}

# 2a. Binary Compilation Target (C++)
# Use this target type if writing the driver in C++.
fuchsia_cc_driver("my-driver-driver") {
  output_name = "my_driver"
  sources = [ "my_driver.cc" ]
  deps = [ ":my-driver-bind" ]
}

# 2b. Binary Compilation Target (Rust)
# Use this target type if writing the driver in Rust. Source file MUST be src/lib.rs.
fuchsia_rust_driver("my-driver-driver") {
  output_name = "my-driver"
  edition = "2024"
  source_root = "src/lib.rs"
  sources = [ "src/lib.rs" ]
  deps = [
    ":my-driver-bind",
    "//sdk/lib/driver/component/rust",
  ]
}

# 3. Driver Component (Same for both, depending on the compiled library target above)
fuchsia_driver_component("my-driver-component") {
  component_name = "my-driver"
  deps = [
    ":my-driver-bind",
    ":my-driver-driver",
  ]
  manifest = "meta/my-driver.cml"
}

# 4. Driver Package (Same for both)
fuchsia_driver_package("my-driver") {
  driver_components = [ ":my-driver-component" ]
}
```

#### Standard Bazel Target Structure

A complete Bazel-based driver `BUILD.bazel` should define the following
equivalent targets (using hyphens/underscores based on parent folder styling):

1.  **`fuchsia_driver_bind_bytecode`**: Compiles the bind rules. Naming
    convention: `[driver-name]-bind`.
2.  **`fuchsia_cc_driver` / `rust_shared_library`**: Compiles the driver binary.
    Naming convention: `[driver-name]-driver`.
3.  **`fuchsia_driver_component`**: Combines the compiled binary and manifest.
    Naming convention: `[driver-name]-component`.
4.  **`fuchsia_package`**: Packages the component. Naming convention should
    match the parent directory exactly: `[driver-name]`.

```bazel
load(
    "@rules_fuchsia//fuchsia:defs.bzl",
    "fuchsia_cc_driver",
    "fuchsia_driver_bind_bytecode",
    "fuchsia_driver_component",
    "fuchsia_package",
)
load(
    "@rules_rust//rust:defs.bzl",
    "rust_shared_library",
)

# 1. Bind Rules (Same for both C++ and Rust)
fuchsia_driver_bind_bytecode(
    name = "my-driver-bind",
    output = "my-driver-bind.bindbc",
    rules = "meta/my-driver.bind",
)

# 2a. Binary Compilation Target (C++)
# Use this target type if writing the driver in C++.
fuchsia_cc_driver(
    name = "my-driver-driver",
    output_name = "my_driver",
    deps = [
        ":my-driver-srcs",
    ],
)

# 2b. Binary Compilation Target (Rust)
# Use this target type if writing the driver in Rust. Source file MUST be src/lib.rs.
rust_shared_library(
    name = "my-driver-driver",
    srcs = [
        "src/lib.rs",
    ],
    edition = "2024",
    deps = [
        "@fuchsia_sdk//pkg/driver_component_rust",
    ],
)

# 3. Driver Component (Same for both, depending on the compiled library target above)
fuchsia_driver_component(
    name = "my-driver-component",
    bind_bytecode = ":my-driver-bind",
    driver_lib = ":my-driver-driver",
    manifest = "meta/my-driver.cml",
)

# 4. Driver Package (Same for both)
fuchsia_package(
    name = "my-driver",
    package_name = "my-driver",
    components = [":my-driver-component"],
)
```

### Add the Test Targets

Tests are usually located in a `tests/` subdirectory or directly in the driver
directory.

* Unit tests often share the directory or are in a dedicated test target in the
  build file.

## Apply Naming Conventions

File names and build targets should follow the convention used by their parent
directory to maintain local consistency.

* **File Names**: Match the style (underscores or hyphens) of the parent
  directory for source, header, and meta files (e.g., `test_driver.cc` if in
  `test_driver/`, or `test-driver.cc` if in `test-driver/`).
* **Build Targets**: Match the style of the parent directory for target names in
  both GN and Bazel build files (e.g., `test_driver` vs `test-driver`). The
  driver package target's name should be the same as the parent directory.

## Common Pitfalls

* **Placing `.bind` outside `meta/`**: While not strictly forbidden by the
  compiler if paths are specified, placing it in `meta/` is the standard
  convention and expected by many templates.
* **Missing `.cml` in `meta/`**: The component manifest must be in `meta/` to be
  correctly processed by component templates.
* **Mismatched Shared Library Name (`output_name`)**: For C++ drivers, if you
  forget to set `output_name` in the `fuchsia_cc_driver` target to match the
  name specified in the manifest binary path, the component runner will fail to
  load the library at runtime (often resulting in a "file not found" error).
* **Missing Dependency on the Bind Target**: Forgetting to add the bind rules
  target (`:my-driver-bind`) to the dependencies of your driver library target
  or component target can cause compilation failures due to missing generated
  bind header files, or prevent binding at runtime.
* **Mismatched Target Naming for Code Generation (Hyphens vs. Underscores)**:
  Forgetting to match the parent folder's style (e.g., using `test_driver_bind`
  instead of `test-driver-bind`) due to an assumption that code-generating
  targets must use underscores. The bind rules target name MUST match the
  hyphenated/underscored styling of the parent folder exactly (e.g.,
  `test-driver-bind` for folder `test-driver/`), otherwise the automated build
  or validation checks will fail to resolve the target.

## Further Reading

* [Debugging Driver Binding](/src/devices/skills/debug_driver_binding/SKILL.md)
  - Workflow for determining why a driver failed to bind.
