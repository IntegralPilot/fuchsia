# Rust Migration Reference

GN `rustc_binary` and `rustc_library` map to Bazel `rustc_binary` and `rustc_library` (loaded from `//build/bazel/rules/rust:defs.bzl`).

## Field Mapping Gotchas

Only key field differences are listed here. Standard fields like `sources` -> `srcs` and `deps` -> `deps` apply normally.

| GN Field                 | Bazel Attribute               | Notes                                                      |
| :----------------------- | :---------------------------- | :--------------------------------------------------------- |
| `output_name`            | `crate_name`                  | The crate name used for linking and resulting binary name. |
| `with_unit_tests = true` | `with_host_unit_tests = True` | Set to `True` to enable host unit tests.                   |
| `features`               | `crate_features`              | Features enabled for this crate.                           |
| `lint_config`            | `lint_config`                 | Target-specific lints config. Same label on both sides.    |

A GN target written as `configs += [ "//build/config/rust/lints:X" ]` should be
migrated to `lint_config = "//build/config/rust/lints:X"` in Bazel. Note that
`lint_config` takes a single label, so a target that needs several lints configs
still has to use `configs` in GN and cannot be migrated as-is.

> **Note:** GN *appends* `lint_config` to the lint configs the `rustc_*()`
> templates already apply, while Bazel *replaces* the macro default with it. The
> configs under `//build/config/rust/lints` are defined to compensate, so both
> spellings produce the same rustc flags for the library target. The generated
> Bazel unit test target does pick up the production lints where GN would only
> apply the default ones, so tests are slightly over-linted in Bazel.


### Third-Party Dependencies

When migrating third-party dependencies from GN to Bazel, prefix with the vendor directory path:

- **GN:** `"//third_party/rust_crates:anyhow"`
- **Bazel:** `"//third_party/rust_crates/vendor:anyhow"`

_Note: Some crates may be located under `ask2patch`, `fork`, or `intree` instead of `vendor` (e.g., `//third_party/rust_crates/ask2patch/walkdir`)._

For other third-party dependencies (e.g. googletest, re2, boringssl), `bazel2gn` translates targets according to [`//build/tools/bazel2gn/third_party_target_map.json`](//build/tools/bazel2gn/third_party_target_map.json).

## Test Migration for `rustc_library()` target

In Bazel, the `rustc_library()` target creates a sub-target named `{target_name}_test` when the `with_host_unit_tests` or `with_unit_tests` attribute is set to `True`.

- **If all dependencies in `test_deps` are migrated to Bazel:**
  1. Register `{target_name}_test` in Bazel:
      * For `//src/developer/ffx/lib/*` libraries, add `//src/developer/ffx/lib/<name>:{target_name}_test` to Bazel `//src/developer/ffx:tests`, and add `"<name>"` to `tests_migrated_ffx_libraries` within `//src/developer/ffx/lib/ffx_libraries_list.gni`.
      * For other rustc libraries, highlight to the user.
  2. Clean up `BUILD.gn` file:
      * Remove `:{target_name}_test` from `group("tests")` in `BUILD.gn`.
      * If `group("tests")` has no remaining dependencies, delete `group("tests")` and remove `":tests"` from the top-level library group.

- **If any dependency in `test_deps` is NOT yet migrated to Bazel:**
  * **Keep** `with_host_unit_tests = True` (or `with_unit_tests = True`) and `test_deps` on `rustc_library()` in `BUILD.bazel` so that `bazel2gn` continues syncing `with_unit_tests = true` to `BUILD.gn`.
  * **Do not** add `{target_name}_test` to Bazel test suites or add the library name to the `tests_migrated_ffx_libraries` list, and **keep** `group("tests")` in `BUILD.gn` so the tests continue running in GN.

## Example

```gn
# BUILD.gn
import("//build/rust/rustc_binary.gni")

if (is_host) {
  rustc_binary("tool_bin") {
    sources = [ "src/main.rs" ]
    edition = "2024"
    deps = [ "//third_party/rust_crates:anyhow" ]
    with_unit_tests = true
    test_deps = [ "//third_party/rust_crates:tempfile" ]
  }
}
```

should be migrated to:

```bazel
# BUILD.bazel
load("//build/bazel/rules/rust:defs.bzl", "rustc_binary")
load("@platforms//host:constraints.bzl", "HOST_CONSTRAINTS")

package(default_applicable_licenses = ["//:license"])

rustc_binary(
    name = "tool_bin",
    srcs = ["src/main.rs"],
    edition = "2024",
    target_compatible_with = HOST_CONSTRAINTS,
    deps = [
        "//third_party/rust_crates/vendor:anyhow",
    ],
    with_host_unit_tests = True,
    test_deps = [
        "//third_party/rust_crates/vendor:tempfile",
    ],
)
```

## Common Pitfalls and Best Practices

### 1. Preventing Redundant Binary Syncs
When `BUILD.gn` is retained (due to unmigrated targets), add `# @bazel2gn:skip` on the line immediately preceding `rustc_binary` in `BUILD.bazel` to prevent `bazel2gn` from generating conflicting GN targets. If all targets in `BUILD.gn` are migrated and `BUILD.gn` is deleted, omit `# @bazel2gn:skip`.

### 2. Preserving GN Target Shape for Unit Tests

Match the GN test structure exactly to ensure correct `bazel2gn` sync:

- **Do not merge** standalone GN `rustc_test` targets into `with_host_unit_tests = True` in Bazel.
- **Do not split** a GN `rustc_library` with `with_unit_tests = true` into separate Bazel library and test targets; use `with_host_unit_tests = True`.

### 3. Specifying Crate Root

If a target has multiple `srcs` and does not use `src/lib.rs` (or `src/main.rs` for binary), explicitly set `crate_root` (e.g., `crate_root = "src/main.rs"`). Otherwise, `bazel2gn` won't generate `source_root` in GN, causing Ninja build errors if GN defaults to `src/lib.rs`.

### 4. Test Data and Genrules

If a host test requires test data:

- **Map Bazel `data` to GN `host_test_data`**: Use annotations to overwrite the path:
  ```bazel
  # @bazel2gn:transformer=deps
  data = [
      ":my_test_data",  # @bazel2gn:path_overwrite::my_gn_test_data
  ],
  ```
- **Skip complex genrules**: `bazel2gn` cannot sync `genrule`s using system commands (like `cp`). Add `# @bazel2gn:skip` above the `genrule` in Bazel and manually maintain the corresponding `host_test_data` in GN.

### 5. `ffx` Subtools and Plugins (`ffx_tool` and `ffx_plugin`)

When migrating Rust subtools and plugins in `//src/developer/ffx/`, use the specialized Starlark macros from `//src/developer/ffx/build/` instead of `rustc_binary_host_tool`:

- **Subtools (`ffx_tool`)** (from `//src/developer/ffx/build:ffx_tool.bzl`):

  ```bazel
  load("@platforms//host:constraints.bzl", "HOST_CONSTRAINTS")
  load("//src/developer/ffx/build:ffx_tool.bzl", "ffx_tool")

  ffx_tool(
      name = "ffx_<subtool>_tool",
      srcs = ["src/main.rs"],
      edition = "2024",
      target_compatible_with = HOST_CONSTRAINTS,
      deps = [
          ":ffx_<plugin>",
          "//src/developer/ffx/lib/fho:lib",
          "//src/lib/fuchsia-async",
      ],
  )
  ```

  - **Target name**: Match the GN target name (e.g. `ffx_sdk_tool`) so `bazel2gn` generates the matching GN target name without mangling.
  - **Host constraints**: Always set `target_compatible_with = HOST_CONSTRAINTS` so `bazel2gn` wraps the target in `if (is_host)` in GN. Supported natively by `bazel2gn` (do not add `# @bazel2gn:skip`).
  - **Plugin dependency**: Depend directly on `:ffx_<plugin>` (the execution library), not `:ffx_<plugin>_suite`.

- **Plugins (`ffx_plugin`)** (from `//src/developer/ffx/build:ffx_plugin.bzl`):

  ```bazel
  load("//src/developer/ffx/build:ffx_plugin.bzl", "ffx_plugin")

  ffx_plugin(
      name = "ffx_<plugin>",
      args_sources = ["src/args.rs"],
      sources = ["src/lib.rs"],
      edition = "2024",
      with_unit_tests = True,
      test_deps = ["//src/lib/fuchsia"],
      deps = ["//src/developer/ffx/lib/fho:lib"],
  )
  ```

  - **Public subtargets**: Exposes `{name}` (execution library), `{name}_suite`, `{name}_args`, and `{name}_config`.
  - **Unit tests**: Use `with_unit_tests = True` and `test_deps = [...]`.
  - **Frontend integration**: For built-in/required plugins, add `"//src/developer/ffx/plugins/<plugin>:ffx_<plugin>"` to `plugin_deps` in `//src/developer/ffx/frontends/ffx/BUILD.bazel`.

- **Cross-toolchain `ffx` libraries (`HOST_CONSTRAINTS`)**:
  Several internal libraries under `//src/developer/ffx/lib/` (e.g., `compat_info`) are built for both host and Fuchsia target devices. Do **not** set `target_compatible_with = HOST_CONSTRAINTS` on cross-toolchain libraries.

#### Verifying `ffx` Migrations

Verify the migrated plugin and subtool using the following pattern:

```bash
# 1. Build host targets (plugin, subtool, and frontend)
fx build --host @//{plugin_path}:{plugin_name}
fx build --host @//{plugin_path}:{subtool_target_name}
fx build --host @//src/developer/ffx/frontends/ffx:ffx

# 2. Run unit tests
fx test {plugin_name}_lib_test

# 3. Test end-to-end execution with Bazel host runner
# Use --isolate-dir to avoid interfering with user config or daemon state
fx bazel run --config=host //src/developer/ffx/frontends/ffx:ffx -- --isolate-dir /tmp/ffx-iso <subcommand>
fx bazel run --config=host //{plugin_path}:{subtool_target_name} <subcommand>
```
