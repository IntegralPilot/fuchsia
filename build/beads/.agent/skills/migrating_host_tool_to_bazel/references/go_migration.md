# Go Migration Reference

## Template Mapping
Migrate GN templates to Bazel following the mapping below.
| GN template  | Bazel rule                                                     |
| ------------ | -------------------------------------------------------------- |
| `go_binary`  | `go_binary_host_tool` (or `idk_go_binary_host_tool` if in IDK) |
| `go_library` | `go_library`                                                   |
| `go_test`    | `host_go_test`                                                 |


## Migration Steps

### Step 1: Migrate `go_library` Target
1. Add `load("@io_bazel_rules_go//go:def.bzl", "go_library")` to the `BUILD.bazel` file if it is not already present.

2. In the GN build file, if test sources (e.g. `*_test.go`) were included in `sources` of `go_library`, separate them out into the `host_go_test` target.

3. Migrate attributes following the mapping below:
| GN field     | Bazel attribute             | Description                                                                             |
| ------------ | --------------------------- | --------------------------------------------------------------------------------------- |
| `sources`    | `srcs`, `embedsrcs`, `data` | GN mixes them; Bazel separates them.                                                    |
| `embedsrcs`  | `embedsrcs`                 | Files accessed via `//go:embed`.                                                        |
| `source_dir` | N/A                         | Not supported in Bazel. Use full relative paths in `srcs`.                              |
| `data`       | `data`                      | Runtime data files.                                                                     |
| `deps`       | `deps`                      | Strict dependency checking: every direct import must be in `deps`.                      |
| `importpath` | `importpath`                | Required in Bazel; must exactly match Go source imports (`go.fuchsia.dev/fuchsia/...`). |

4. Scope visibility explicitly (e.g., `visibility = ["//tools/<pkg>:__subpackages__"]` or package-private). Avoid `package(default_visibility = [...]` and `default_visibility = ["//visibility:public"])`.


### Step 2: Migrate `go_binary` Target
Attributes mapping table for `go_binary` targets:
| GN field     | Bazel attribute             | Description                                                       |
| ------------ | --------------------------- | ----------------------------------------------------------------- |
| `sources`    | `srcs`, `embedsrcs`, `data` | GN mixes them; Bazel separates them.                              |
| `embedsrcs`  | `embedsrcs`                 |                                                                   |
| `source_dir` | N/A                         | Not supported in Bazel. Use full relative paths in `srcs`.        |
| `data`       | `data`                      |                                                                   |
| `deps`       | `deps`                      | Direct deps only. Do NOT over-declare deps of embedded libraries. |
| `embed`      | `embed`                     | Embeds internal library target (e.g. `[":main"]` or `[":lib"]`).  |

1. Refer to [target_compatible_with.md](target_compatible_with.md) to identify if the target is a tool in the IDK.
   **For tools not in the IDK:**
   - Add `load("//build/bazel/rules/host:defs.bzl", "go_binary_host_tool")` to `BUILD.bazel`.
   - Migrate GN `go_binary` to `go_binary_host_tool()`.

   **For tools in the IDK:**
   - Add `load("//build/bazel/rules/idk:idk_host_tool.bzl", "idk_go_binary_host_tool")` to `BUILD.bazel`.
   - Migrate GN `go_binary` to `idk_go_binary_host_tool` following both the "Attributes mapping table for `go_binary` targets" table above and the IDK attributes mapping table for IDK specific attributes (`api_area`, `idk_name`, `category`).

   IDK attributes mapping table.
   | GN field        | Bazel attribute  | Description                                                     |
   | --------------- | ---------------- | --------------------------------------------------------------- |
   | see description | `api_area`       | `sdk_area` in corresponding `sdk_host_tool()` GN target.        |
   |                 |                  | If not provided, select an area based on the definitions within |
   |                 |                  | //docs/contribute/governance/areas/_areas.yaml.                 |
   | see description | `idk_name`       | `sdk_name` in corresponding `sdk_host_tool()` GN target.        |
   |                 |                  | If not provided, use the same name with the GN target.          |
   | see description | `category`       | `category` in corresponding `sdk_host_tool()` GN target.        |

2. **Visibility:** Binary targets default to package-private (omit `visibility` unless external Bazel packages depend on them).


### Step 3: Migrate `go_test` Target to `host_go_test`
1. Add `load("//build/bazel/rules/host_tests:host_go_test.bzl", "host_go_test")` and `load("@platforms//host:constraints.bzl", "HOST_CONSTRAINTS")` to `BUILD.bazel`.

2. Migrate the `go_test` target to `host_go_test`:
   - Set `srcs` to test Go source files (e.g. `["*_test.go"]` or specific test files).
   - Set `embed = [":lib"]` (or `[":main"]`) to embed the library under test.
   - Set `deps` to test-specific dependencies (e.g., `//third_party/golibs:github.com/google/go-cmp/cmp`, `//third_party/golibs:github.com/google/go-cmp/cmp/cmpopts`).
   - Set `visibility = ["//tools:__pkg__"]` (or the parent package containing the top-level `host_tests` suite).


### Step 4: Host Test Integration
1. **Do NOT create intermediate package-level `test_suite(name = "tests", ...)` targets** inside each tool's `BUILD.bazel`.
2. Integrate individual `host_go_test` target(s) directly into the top-level `"host_tests"` `test_suite` in the parent or ancestor directory (e.g., `//tools:host_tests` in `//tools/BUILD.bazel` or `//build/tools:host_tests` in `//build/tools/BUILD.bazel`).
3. **Remove migrated tests from GN:** Remove the legacy test target from the parent GN test group (e.g., `group("tests_no_e2e")` in `//tools/BUILD.gn` or `group("tests")` in `//build/tools/BUILD.gn`). Ensure unmigrated GN tests remain in GN.


### Step 5: Add `target_compatible_with` Attribute
1. See [target_compatible_with.md](target_compatible_with.md) to set `target_compatible_with` to correct constraint values.


### Step 6: Separate Non-Go Sources
In Bazel, separate sources into appropriate attributes:
- `srcs`: `.go`, `.s`, `.syso` (and C/C++ sources if `cgo = True`).
- `embedsrcs`: Files used with `//go:embed` in `.go` files.
- `data`: Runtime data files.


## Common Pitfalls and Best Practices

### 1. `importpath` Alignment
The `importpath` in `go_library` **must match exactly** the string used in Go `import` statements (`go.fuchsia.dev/fuchsia/...`).
- **Pitfall:** If you have multiple targets in the same directory (e.g., `proto_lib`, `cmd`), automatic generation might append target names (e.g., `importpath = ".../tool/proto_lib"`).
- **Fix:** Check `package` statements and dependent `import` blocks to align `importpath` explicitly.

### 2. Strict Dependency Chains
Bazel enforces strict dependency checking for Go compile steps.
- **Failure mode:** `compilepkg: missing strict dependencies: import of "..."`
- **Fix:** Every package imported in source files must have its corresponding Bazel target explicitly listed in `deps`.

### 3. Over-declaring Transitive Dependencies on Binary Targets
- **Pitfall:** Redundantly listing the dependencies of internal libraries (e.g., `:lib` or `:main`) in `deps` of `go_binary_host_tool`.
- **Rule:** In Bazel (`rules_go`), each target only needs to declare the direct imports required by its own `srcs`. When a binary target depends on or embeds a library, transitive dependencies are automatically resolved and linked by Bazel and must not be duplicated in the binary's `deps`.

### 4. Visibility Management
- **Pitfall:** Setting `package(default_visibility = ["//visibility:public"])` or exposing internal test targets repository-wide.
- **Rule:**
* Binary targets default to package-private (omit `visibility` unless external Bazel packages depend on them).
* Library targets default to package-private unless they are needed by subpackages (omit `visibility` unless subpackages depend on them).

### 5. Host Tool Installation Flags
- **Pitfall:** Omitting `install_host_tool = true` for tools wrapped by `install_host_tools` in GN.
- **Rule:** Set `install_host_tool = true` in the `bazel_host_tool` target only if `install_host_tools()` was present in GN.

### 6. Improper `BUILD.gn` Deletion & `bazel2gn` Handling
- **Pitfall:** Deleting `BUILD.gn` when unmigrated or externally referenced GN targets still remain, OR retaining `verify_bazel2gn` targets when `BUILD.gn` has been deleted.
- **Rule:**
  - If all targets in original `BUILD.gn` are migrated and not referenced by targets in other GN build file (and not in `host_labels`): omit `# @bazel2gn:skip`, and do NOT add `verify_bazel2gn` to `bazel2gn_verification_targets.gni`.
  - If any migrated target in `BUILD.gn` is referenced by targets in other GN build files: add `# @bazel2gn:skip` before the binary in `BUILD.bazel`, sync remaining targets with `bazel2gn`, and keep `"//{dir}:verify_bazel2gn"` in `//build/bazel2gn_verification_targets.gni`.

### 7. Creating Unnecessary Package-Level Test Suites
- **Pitfall:** Defining an intermediate `test_suite(name = "tests", ...)` inside each tool's package `BUILD.bazel`.
- **Rule:** Do NOT create package-level `test_suite` targets for Go host tools. Include the individual `host_go_test` target(s) directly into the top-level `//tools:host_tests` suite in `//tools/BUILD.bazel`, and restrict test target visibility to `visibility = ["//tools:__pkg__"]`.

### 8. Test Dependencies After Library Sync (When `BUILD.gn` is Retained)
- **Pitfall:** When `BUILD.gn` is retained and `bazel2gn` syncs `go_library` back to GN without test files, the synced GN library will not depend on test-only libraries.
- **Fix:** Manually add missing test dependencies to `deps` of the `go_test` target in `BUILD.gn`.
