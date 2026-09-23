# Host Tool to Bazel Migration Review Guidelines

This document outlines the criteria, standards, and rule-based checks for validating code changes that migrate host tools (Go/Rust host binaries, libraries, and host tests) from GN to Bazel. It is designed to be loaded as context for an automated code review agent.

---

## 1. Agent Reviewer Scope & Role

When reviewing a CL that migrates host tools from GN to Bazel:

- **Role:** Assume the role of a Fuchsia build system expert, knowledgeable in GN, Bazel, and how to migrate build targets between the two systems.
- **Goal:** Ensure target parity, licensing, visibility scoping, correct root target registration, root target references in dependent GN targets, proper `bazel2gn` synchronization setup, dependency strictness, formatting, and commit message.

---

## 2. Rule-Based Review Checks

### Rule 2.1: Copyright Header, Licensing & Naming

- **Copyright Header:** Newly created `BUILD.bazel` files MUST include standard Fuchsia copyright headers with the current year (or original upload year).
- **Default Applicable Licenses:** Newly created `BUILD.bazel` files MUST include `package(default_applicable_licenses = ["//:license"])`, placed after any `load(...)` statements to comply with Bazel syntax and repository licensing rules.
- **Target Name Parity:** The target name defined in `BUILD.bazel` MUST match the legacy target name in `BUILD.gn`.

### Rule 2.2: Target Compatibility (`target_compatible_with`)

- **Standard Host Tools:** MUST set `target_compatible_with = HOST_CONSTRAINTS` (loaded from `@platforms//host:constraints.bzl`).
- **IDK Host Tools:** MUST set `target_compatible_with = HOST_OS_CONSTRAINTS` (loaded from `@platforms//host:constraints.bzl` or `//build/bazel/platforms:constraints.bzl`).

**GOOD:**

```bazel
load("@platforms//host:constraints.bzl", "HOST_CONSTRAINTS")
load("//build/bazel/rules/rust:defs.bzl", "rustc_binary")

package(default_applicable_licenses = ["//:license"])

rustc_binary(
    name = "my_tool",
    target_compatible_with = HOST_CONSTRAINTS,
    ...
)
```

**BAD:** Missing `target_compatible_with` or omitting the `load` statement for `HOST_CONSTRAINTS`.

---

### Rule 2.3: Visibility Scoping

- **Package-Level Visibility:** Avoid setting default visibility on the package level (`package(default_visibility = [...])`).
- **Target-Level Visibility:** Set target-level `visibility` as restrictively as possible on individual targets (e.g., restrict visibility to specific packages that require access rather than `"//visibility:public"`) to prevent unintended dependencies across packages.

---

### Rule 2.4: Root Target Registration (`bazel_root_targets_list.gni`)

- **List Choice:**
  - Tools under `//tools` MUST be added to `tools_bazel_root_targets` in `//tools/bazel_root_targets_list.gni`.
  - All other host tools MUST be added to `default_bazel_root_host_targets` in `//build/bazel/bazel_root_targets_list.gni`.
- **`copy_outputs` Configuration:**
  - Go binaries output to `{{BAZEL_TARGET_OUT_DIR}}/{target_name}_/{target_name}` and MUST specify `copy_outputs`.
- **`install_host_tool` Field:**
  - If wrapped with `install_host_tools` in GN, set `install_host_tool = true` in the root targets entry and remove the old `install_host_tools` wrapper from `BUILD.gn`.

**GOOD:**

```gn
{
  bazel_label = "//tools/my_tool:my_tool"
  copy_outputs = [
    {
      bazel = "{{BAZEL_TARGET_OUT_DIR}}/my_tool_/my_tool"
      ninja = "my_tool"
    },
  ]
  install_host_tool = true
}
```

---

### Rule 2.5: GN References Update

- Dependent GN targets MUST be updated from legacy GN target labels (e.g., `//tools/my_tool:my_tool`) to the Bazel root target wrapper:
  `//build/bazel/host:bazel_root_host_tools.{target_name}`

---

### Rule 2.6: Synchronizer (`bazel2gn`) & Verification Targets

- **Deleting `BUILD.gn` When Fully Migrated:**
  - If all targets in `{directory_path}/BUILD.gn` are migrated to Bazel and no external GN targets depend on them, delete `BUILD.gn` directly.
  - Do NOT run `bazel2gn`, do NOT add `# @bazel2gn:skip`, and do NOT add `"//{directory_path}:verify_bazel2gn"`.
- **`# @bazel2gn:skip` Directive:**
  - MUST add `# @bazel2gn:skip` on the line immediately preceding `go_binary_host_tool` or `rustc_binary` in `BUILD.bazel` IF a `BUILD.gn` file remains in that directory for synced libraries/tests.
  - MUST REMOVE (or not add) `# @bazel2gn:skip` if the `BUILD.gn` file is completely deleted.
- **Verification List (`bazel2gn_verification_targets.gni`):**
  - Directories containing synced libraries or tests MUST have `"//{directory_path}:verify_bazel2gn"` added to `bazel2gn_verification_targets` in `//build/bazel2gn_verification_targets.gni`.
  - Do NOT add `"//{directory_path}:verify_bazel2gn"` if `BUILD.gn` is deleted.
  - Entries MUST preserve alphabetical sorting inside the `# keep-sorted` block.

**GOOD:**

```bazel
# @bazel2gn:skip
go_binary_host_tool(
    name = "my_tool",
    ...
)
```

---

### Rule 2.7: Language-Specific Guidelines

#### Go Host Tools

- **Rule Imports:**
  - `go_library` from `@io_bazel_rules_go//go:def.bzl`
  - `go_binary_host_tool` from `//build/bazel/rules/host:defs.bzl` (do NOT import standard `go_binary`)
  - `host_go_test` from `//build/bazel/rules/host_tests:host_go_test.bzl`
- **`importpath` Alignment:** `importpath` in `go_library` MUST match the exact package import string used in dependent `.go` files.
- **Strict Dependencies:** Local package dependencies must be explicitly declared in `deps`.
- **Source Separation:** Explicitly separate `srcs`, `embedsrcs`, and `data`.

#### Rust Host Tools

- **Rule Imports:** `rustc_binary` and `rustc_library` from `//build/bazel/rules/rust:defs.bzl`.
- **Edition:** Must use `edition = "2024"`.
- **Field Mappings:**
  - `output_name` in GN -> `crate_name` in Bazel.
  - `with_unit_tests = true` in GN -> `with_host_unit_tests = True` in Bazel.
  - `features` in GN -> `crate_features` in Bazel.
- **Third-Party Dependencies:** Third-party crate references MUST use the Bazel vendor path (e.g., `//third_party/rust_crates/vendor:anyhow`).

---

### Rule 2.8: Host Test Suite & Registration

- **Package Test Suite:** Migrated test targets SHOULD be grouped under a package-level `"tests"` `test_suite()` target with `visibility` restricted to the parent/ancestor directory containing the root `test_suite()` (e.g., `visibility = ["//tools:__pkg__"]` or `visibility = ["//build/tools:__pkg__"]`).
- **Root Host Tests Registration:** When migrating host tests, `"//{directory_path}:tests"` MUST be added to the parent/ancestor `"host_tests"` `test_suite()` target (e.g., the `//tools:host_tests` `test_suite()` target in `//tools/BUILD.bazel` or `//build/tools:host_tests` in `//build/tools/BUILD.bazel`) so the tests are automatically included in centralized CI test suites.
- **Do Not Duplicate in GN:** Migrated Bazel host tests should be Bazel-only. They are automatically included in CI/CQ when added to the parent `test_suite()` target above. Do NOT add them to GN `group("tests")`, and ensure any old GN test references for migrated tests are removed from `group("tests_no_e2e")` in `//tools/BUILD.gn` (or equivalent in `//build/tools/BUILD.gn`).
- **Reviewer Check for Test Parity:** Reviewers must ensure all tests removed from `BUILD.gn` have matching definitions in a `BUILD.bazel` file that is included in a Bazel `test_suite()` named `"tests"`. Tests defined in GN that cannot yet be migrated must remain in GN.

**GOOD:**

```bazel
# In //tools/my_tool/BUILD.bazel
test_suite(
    name = "tests",
    tests = [":my_tool_tests"],
    visibility = ["//tools:__pkg__"],
)

# In //tools/BUILD.bazel
test_suite(
    name = "host_tests",
    tests = [
        ...
        "//tools/my_tool:tests",
    ],
)
```

**BAD:** Migrating host tests without including them in a `"tests"` `test_suite()`, without exposing and registering that `"tests"` target in the parent/ancestor `"host_tests"` `test_suite()` target, duplicating the host test in GN test groups, or deleting un-migrated GN tests.

---

## 3. Commit Message Requirements

- **Subject Line:** MUST include the `[bazel_migration]` tag (e.g., `[bazel_migration] Migrate my_tool host binary to Bazel`).
- **Bug Footer:** MUST include `Bug: <issue-id>`.
- **Test Footer:** MUST include `Test:` footer describing how the migration was verified.
