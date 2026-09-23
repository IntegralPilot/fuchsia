---
name: migrating-host-tool-to-bazel
description: >-
  Migrates Fuchsia host tools from GN to Bazel build systems following the
  standard migration framework. Use when converting go_binary, go_library,
  go_test, rustc_library, or rustc_binary targets from BUILD.gn to BUILD.bazel,
  adding bazel_host_tool target for migrated tools , updating infrastructure
  builder configs (shared.star), integrating host tests directly into
  //tools:host_tests, syncing Bazel outputs back to GN using bazel2gn when
  unmigrated targets remain, or decommissioning obsolete BUILD.gn files.
---

This skill defines the standard 6-phase framework for migrating host tools (Go and Rust) from GN to Bazel in the Fuchsia codebase.

## Phase 1: Pre-Migration & Dependency Verification

1. **Confirm Host Tool Target:**
   Verify that the target is a host tool target:

   ```bash
   fx build --host //{directory_path}:{target_name}
   ```

2. **Inspect Dependency Tree:**
   Get the full dependency tree of the host tool target:

   ```bash
   # <target_label> e.g. "//path/to/directory:target_name"
   fx gn desc $(fx get-build-dir) "<target_label>(//build/toolchain:host_x64)" deps --tree
   ```

   Ensure all dependencies of the host tool target are buildable in Bazel. If not, recursively migrate missing dependencies first using this skill:

   ```bash
   # Keep the @ prefix when building Bazel targets with `fx build`.
   fx build --host @//{dependency_path}:{dependency_name}
   ```

3. **Check Existing Bazel Targets:**
   If `BUILD.bazel` already exists, check if the actual binary targets have already been migrated. If so, warn the user and stop the migration process.

## Phase 2: Author BUILD.bazel Targets

1. Create a `BUILD.bazel` file in the same directory as the `BUILD.gn` of the migrated target.

2. Refer to `assets/build_bazel_header_template.md` to add the copyright header and `package(default_applicable_licenses = ["//:license"])` declaration.
   - **NOTE:** Place `package(...)` after any `load(...)` statements, as required by Bazel syntax.

3. Refer to language-specific guides and examples to create bazel targets in
   the BUILD.bazel file.
   - [Go Migration Guide](references/go_migration.md)
   - [Rust](references/rust_migration.md)

## Phase 3: Add bazel_host_tool() target

In `BUILD.gn`, define a `bazel_host_tool()` target using the same name as the `go_binary()` target.

The parameters of the `bazel_host_tool()` target should be set as below.

- Set `bazel_target` to a string formatted as `":<target_name>"`.
- Set `bazel_output_path` based on the underlying language-specific Bazel rule:
  - C/C++ or Rust : `"{{BAZEL_TARGET_OUT_DIR}}/<target_name>"`.
  - Go : `"{{BAZEL_TARGET_OUT_DIR}}/<target_name>_/<target_name>"`.
- If the migrated target was previously wrapped in an `install_host_tools` target, set the `install_host_tool` attribute to `true`.

## Phase 4: Evaluate GN Cleanup & Verification Targets

Check remaining GN references in `{directory_path}`:

```bash
fx gn refs $(fx get-build-dir) "//{directory_path}/*"
```

- **Case 1: All targets in `{directory_path}/BUILD.gn` are migrated / unreferenced (and not in `host_labels`):**
  1. **Do NOT sync back from Bazel to GN:** Do NOT run `bazel2gn`, do NOT add `# @bazel2gn:skip` in `BUILD.bazel`, and do NOT add `verify_bazel2gn` to `//build/bazel2gn_verification_targets.gni`.
  2. Run `fx gen` to validate the GN build graph.

- **Case 2: Unmigrated GN targets remain or external GN targets still depend on libraries in `{directory_path}`:**
  1. Remove the migrated targets from `{directory_path}/BUILD.gn`.
  2. **Prevent redundant binary syncs:** Add `# @bazel2gn:skip` on the line immediately preceding `go_binary_host_tool` or `rustc_binary` in `BUILD.bazel` so they aren't generated in GN (see language guides for exceptions like `ffx_tool`).
  3. **Sync back to GN:** Sync required library targets back from Bazel to GN using `syncing-bazel-to-gn` (see `../syncing_bazel_to_gn/SKILL.md`).
  4. **Add verification target:** Add `"//{directory_path}:verify_bazel2gn"` to the `bazel2gn_verification_targets` list in `//build/bazel2gn_verification_targets.gni` (or `//sdk/fidl/bazel2gn_verification_targets.gni` for FIDL).
  5. Run `fx gen` to validate the GN build graph.
  6. **Clean up redundant GN targets:** In the synced `BUILD.gn`, remove library targets that are not referenced by other GN targets. If no targets are referenced by external targets, remove `# @bazel2gn:skip` from `BUILD.bazel`, and remove `"//{directory_path}:verify_bazel2gn"` from `bazel2gn_verification_targets.gni`.

## Phase 5: Verification, Formatting & Full Build Check

Execute the following verification steps in order:

```bash
# 1. Regenerate GN build files to validate graph consistency
fx gen

# 2. Run Bazel rule quick tests
fx build //build/bazel/rules/tests:quick_tests

# 3. Verify direct Bazel build for migrated tools
fx build --host @//{directory_path}:{target_name}

# 4. Build individual Bazel-in-GN host tools
fx build --host //{directory_path}:{target_name}

# 5. Run migrated host tests individually under Bazel
fx bazel test --config=host //{directory_path}:{test_target_name}

# 6. Verify aggregated host test suite directly under Bazel
fx bazel test --config=host //tools:host_tests

# 7. Format all modified files
fx format-code --parallel

# 8. Final full build check across repository graph
fx set fuchsia.x64 --main-pb //products/core:product_bundle.x64 && fx build

# 9. Build direct Bazel target (__ONLY__ for cross-toolchain libraries)
fx bazel build --config=fuchsia_platform //{directory_path}:{target_name}

# 10. Verify all bazel2gn drift check targets across the tree
fx build --host //build:bazel2gn_verifications
```
