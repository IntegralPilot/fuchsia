---
name: migrate-cpp-to-rust-scaffolding
description: >
  Scaffold (do not implement) the conversion of a Fuchsia C++ driver to Rust
  by generating the coexisting -rust target, src/lib.rs boilerplate, the
  meta/*-rust.cml manifest, BUILD.gn driver/test groups, and the
  all_drivers_list.txt entry via scaffold_migration.py. Use when starting a
  C++-to-Rust migration and wanting the initial setup, build wiring, and
  optional C++/Rust hot-swap feature flag automated. Only scaffolds -- don't
  use for the actual logic port or manual code conversion (use migrate-cpp-to-
  rust-driver), or for drivers already migrated.
---

# Scaffolding for Converting a C++ Driver to Rust

## Goals and Strategy

Based on the migration goals:
- **Coexistence**: The C++ driver remains available while the Rust driver is
  developed.
- **Naming**: To avoid collisions and allow building both, the Rust driver
  targets and files use a `-rust` suffix (e.g., `my-driver-rust`).
- **Localization**: Code changes are localized to the driver's directory.

## Automated Steps

Use the provided script to automate the creation of the Rust target (derived
from `tools/create` templates), boilerplate files, and adding the driver to
[all_drivers_list.txt](/build/drivers/all_drivers_list.txt).

### Use the Scaffolding Script

Run the script from the Fuchsia root directory:

```bash
./src/devices/skills/migrate_cpp_to_rust_scaffolding/scripts/scaffold_migration.py \
  --name <driver-name> \
  --dir <relative-path-to-driver-dir> \
  --bind-target <bind-target-label>
```

Example:
```bash
./src/devices/skills/migrate_cpp_to_rust_scaffolding/scripts/scaffold_migration.py \
  --name my-driver \
  --dir src/devices/misc/drivers/my-driver \
  --bind-target :my_driver_bind
```

The script performs the following actions:
1.  Creates `src/lib.rs` with basic driver registration (if it doesn't exist).
2.  Creates `meta/<driver-name>-rust.cml` component manifest.
3.  Appends the Rust driver, component, and test targets to `BUILD.gn` (using
    `-rust` suffixes and `with_unit_tests = true`).
4.  Updates the local `tests` group in `BUILD.gn` to include the new test
    package.
5.  Updates the local `drivers` group in `BUILD.gn` to include the new driver
    package (creates it if missing).
6.  Updates the parent directory's `tests` group to include the child
    directory's tests.
7.  Updates the parent directory's `drivers` group to include the child
    directory's `drivers` group.
8.  Adds the driver component label to
    [all_drivers_list.txt](/build/drivers/all_drivers_list.txt).
9.  Runs `fx format-code` on all created or modified files in the driver
    directory.

## Post-Scaffolding Steps

After running the script, perform the following manual steps to complete the
setup:

### 1. Add Driver Info

The `fuchsia_driver_component` target requires an `info` field pointing to a
JSON file containing driver metadata (e.g., `meta/component-info.json`). Add
this field to the generated target in `BUILD.gn`. If the C++ driver already has
an info file, reuse it.

Example:
```gn
fuchsia_driver_component("my-driver-rust-component") {
  ...
  info = "meta/my-driver-info.json"
}
```

### 2. Verify Build

Run `fx build` to ensure both drivers compile correctly.

### 3. Add Tests

Add tests for the Rust driver and include them in CQ/CI.

> [!IMPORTANT]
> If you add a new test target (e.g., using `fuchsia_unittest_package`), you must run `fx add-test <path-to-target>` and then a full `fx build` before running `fx test`. Failure to do so may result in `fx test` failing to find the test or component resolution errors (e.g., "Could not find a Merkle hash").

## Advanced Scaffolding: Parameterizing for Hot Swapping

To allow a developer to swap the C++ driver for the Rust driver without
modifying any board configuration, you can parameterize the target names and
manifests in `BUILD.gn`.

### Step A: Define variables at the top of `BUILD.gn`

```gn
declare_args() {
  # Set to true to use the Rust implementation of the driver.
  use_my_driver_rust = false
}

if (use_my_driver_rust) {
  cpp_component_name = "my-driver-component-cpp"
  cpp_package_name = "my-driver-cpp"
  cpp_output_name = "my-driver-cpp"
  cpp_manifest = "meta/my-driver-cpp.cml"

  rust_component_name = "my-driver-component"
  rust_package_name = "my-driver"
  rust_output_name = "my-driver"
  rust_manifest = "meta/my-driver-rust-active.cml"
} else {
  cpp_component_name = "my-driver-component"
  cpp_package_name = "my-driver"
  cpp_output_name = "my-driver"
  cpp_manifest = "meta/my-driver.cml"

  rust_component_name = "my-driver-rust-component"
  rust_package_name = "my-driver-rust"
  rust_output_name = "my-driver-rust"
  rust_manifest = "meta/my-driver-rust.cml"
}
```

### Step B: Keep Binary Target Names Static

To satisfy `all_drivers_list.txt` validation, keep the `fuchsia_cc_driver` and
`fuchsia_rust_driver` target names static (e.g., `my-driver` and
`my-driver-rust-lib`), but use the parameterized `cpp_output_name` and
`rust_output_name` to control the actual binary filename.

### Step C: Use 4 Manifests

Create 4 manifests to handle the binary name mapping:
1.  `meta/my-driver.cml`: Points to `my-driver.so` (C++ active).
2.  `meta/my-driver-cpp.cml`: Points to `my-driver-cpp.so` (C++ inactive).
3.  `meta/my-driver-rust.cml`: Points to `my-driver-rust.so` (Rust inactive).
4.  `meta/my-driver-rust-active.cml`: Points to `my-driver.so` (Rust active).

Update the `fuchsia_driver_component` targets to use the parameterized
`cpp_manifest` and `rust_manifest`.

## Switching Between C++ and Rust Drivers

For instructions on how to dynamically switch between the C++ driver and the
Rust driver at runtime, see the [Driver Hot
Reload](/src/devices/skills/driver_hot_reload/SKILL.md) skill.

## Common Pitfalls

* **Naming Collisions**: Ensure the Rust driver uses the `-rust` suffix to avoid
  conflicts with the C++ driver.

