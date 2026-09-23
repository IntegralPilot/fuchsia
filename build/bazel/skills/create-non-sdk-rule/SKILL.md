---
name: create-non-sdk-rule
description: >-
  Instructions for creating non-SDK versions of Bazel build rules that
  currently exist in the SDK.
---

# Creating Non-SDK In-Tree Bazel Rules

This guide outlines how to refactor and create in-tree ("non-SDK") versions of
Fuchsia Bazel build rules. Non-SDK rules allow the platform build to use host
tools built locally from the active GN workspace (via the `@gn_targets//`
repository) instead of prebuilt binaries from the Fuchsia SDK.

> [!IMPORTANT]
> A key benefit of this pattern is that both the SDK-based and non-SDK versions
> of the rules return and consume the exact same Bazel providers. This allows
> targets built using different rule variants to seamlessly "talk to each other"
> and integrate without translation layers.

> [!IMPORTANT]
> **Dependency Boundaries**:
> Non-SDK (in-tree) rules defined under `//build/bazel/rules/...` **CANNOT**
> load or depend on Starlark files or tools defined inside `@rules_fuchsia`.
> All shared providers, helper functions, aspect rules, and Python tools must
> reside entirely within `@fuchsia_rules_common` so both rule variants can
> reference them without creating circular or invalid dependency layers.

---

## The 5-Step Recipe

Creating an in-tree, non-SDK version of an existing SDK rule involves the
following phases:

### Step 1: Move & Parameterize Core Implementation in `fuchsia_rules_common`
To ensure identical interfaces and avoid duplication, both providers and
implementation logic must be shared:
1. Move or define the necessary Bazel providers (e.g., `FuchsiaPackageInfo`) in
   the shared `fuchsia_rules_common` repository (typically under
   `//build/bazel_sdk/fuchsia_rules_common/<category>/providers.bzl`) so both
   SDK and non-SDK rules can import the same provider types.
   * **Note**: If you introduce a new subdirectory/category under
     `fuchsia_rules_common`, you must expose its `.bzl` files (using a
     `bzl_srcs` `filegroup` target in the subdirectory's `BUILD.bazel`) and
     register it in the top-level
     `//build/bazel_sdk/fuchsia_rules_common/BUILD.bazel` `bzl_srcs` target:
     ```starlark
     filegroup(
         name = "bzl_srcs",
         srcs = [
             "local_actions.bzl",
             "@fuchsia_rules_common//<new_category>:bzl_srcs",
             # ...
         ],
     )
     ```
2. Locate the SDK rule's implementation function (typically under
   `//build/bazel_sdk/bazel_rules_fuchsia/fuchsia/private/`).
3. Extract the core execution and action-declaration logic into a parameterized
   helper function (e.g., `common_xxx_impl`) in a `.bzl` file within the
   `fuchsia_rules_common` repository (e.g.,
   `//build/bazel_sdk/fuchsia_rules_common/`).
4. Parameterize the execution tools (e.g., `package_tool` or
   `assembly_config_binary`) instead of referencing `sdk.ffx` directly. Use
   configuration flags (e.g., `package_tool_is_ffx = True/False`) to handle
   differences in tool execution environments (e.g., FFX isolation
   requirements).
5. For any shared helper functions that are relocated to `fuchsia_rules_common`
   as part of the refactoring, remove their original definitions from the SDK's
   utility files (e.g., `private/utils.bzl`). Load them from the new shared
   location and re-export them so all other pre-existing usages in the SDK
   continue to work without modifications.
6. Group and export the rule's common attributes into a shared Starlark
   dictionary (e.g., `COMMON_XXX_ATTRIBUTES = { ... }`) within the same
   `fuchsia_rules_common` file.
7. Move any helper Python scripts required by the rule into the
   `fuchsia_rules_common` repository. These scripts should be referenced and
   used directly from `@fuchsia_rules_common` in the shared implementation
   instead of being passed through SDK-specific or non-SDK-specific attributes.

### Step 2: Adapt the SDK Rule to Use the Common Implementation
Update the original SDK-facing rule definition:
1. Load `COMMON_XXX_ATTRIBUTES` and the helper function `common_xxx_impl` from
   `@fuchsia_rules_common`.
2. Implement the SDK rule's attributes by merging the common attributes with
   any SDK-specific attributes using Starlark's dictionary union:
   ```starlark
   attrs = COMMON_XXX_ATTRIBUTES | {
       # SDK-specific attributes go here
   }
   ```
3. Inside the SDK rule's implementation function, extract the tools from the SDK
   provider (e.g., `sdk.ffx_package`) and forward them to the common helper
   function:
   ```starlark
   def _unpack_prebuilt_package_impl(ctx):
       return unpack_prebuilt_package_impl(
           ctx,
           package_tool = sdk.ffx_package,
           package_tool_is_ffx = True,
           packaged_components = _make_component_info(ctx),
       )
   ```

### Step 3: Create the Non-SDK In-Tree Rule
Define the new in-tree rule in the Fuchsia repository (typically under
`//build/bazel/rules/`):
1. Load `COMMON_XXX_ATTRIBUTES` and the helper function `common_xxx_impl` from
   `@fuchsia_rules_common`.
2. Declare the new rule, defining private implicit attributes (prefixed with
   `_`) that default to host tools:

   > [!IMPORTANT]
   > **Prefer Bazel-Compiled Tools**: If the host tool is already compiled
   > directly in Bazel (i.e., it has a `BUILD.bazel` file and can be built
   > within the Bazel workspace), you should reference its Bazel target
   > directly (e.g., `//src/sys/pkg/bin/package-tool`). The GN-compiled
   > version of the tool (resolved via the `@gn_targets//` repository) should
   > only be used if the tool cannot be compiled in Bazel.

   ```starlark
   prebuilt_package = rule(
       implementation = _prebuilt_package_impl,
       attrs = COMMON_XXX_ATTRIBUTES | {
           "_package_tool": attr.label(
               default = "@gn_targets//toolchain_host_x64/src/sys/pkg/bin/package-tool",
               executable = True,
               cfg = "exec",
           ),
       },
   )
   ```
3. Implement the rule's implementation function, retrieving the executable
   from the private attribute using `ctx.executable._attribute_name` and
   forwarding it to the common helper:
   ```starlark
   def _prebuilt_package_impl(ctx):
       return unpack_prebuilt_package_impl(
           ctx,
           package_tool = ctx.executable._package_tool,
       )
   ```
4. Wrap the rule inside a public Starlark macro if you need to pre-process
   inputs (e.g., parsing or encoding JSON configurations, setting up default
   dictionary structures, etc.).

### Step 4: Expose the Host Tool to Bazel via GN

> [!IMPORTANT]
> **Prefer Bazel-Compiled Tools**: If the host tool is already compiled
> directly in Bazel, you should skip this step entirely. This step of
> exposing host binaries via `bazel_input_file` or `bazel_input_directory`
> is only required for GN-compiled host tools that do not support native
> Bazel compilation.

To make the in-tree host tool accessible under `@gn_targets//`, you must
expose the host binary build target as a Bazel input:
1. Open the `BUILD.gn` file defining the host tool (e.g.,
   `//src/sys/pkg/bin/package-tool/BUILD.gn`).
2. Import the bazel input template:
   ```gn
   import("//build/bazel/bazel_inputs.gni")
   ```
3. Define a `bazel_input_file` or `bazel_input_directory` target to package the
   built binary:
   ```gn
   bazel_input_file("bazel_input") {
     generator = ":package-tool"
     outputs = [ "${root_out_dir}/package-tool" ]
   }
   ```

### Step 5: Update In-Tree BUILD Usages (User-Managed)
Transitioning existing target usages from the old SDK rules to the new in-tree
rules should be done on a case-by-case basis by the user:
1. Locate references to the old SDK rule (loaded from
   `@rules_fuchsia//fuchsia:...`).
2. Change the `load` statements to import the new non-SDK rule (loaded from
   `//build/bazel/rules/...`).

---

## Reference Examples

Rather than copying large, static Starlark definitions, refer directly to these
production implementations in the tree to see how the patterns are applied in
practice:

### Example A: Prebuilt Package Extraction (`prebuilt_package`)
This rule extracts a prebuilt Fuchsia package archive (`.far`).
* **Common Providers**: [providers.bzl](//build/bazel_sdk/fuchsia_rules_common/packages/providers.bzl)
* **Common Implementation**: [prebuilt_package.bzl](//build/bazel_sdk/fuchsia_rules_common/packages/prebuilt_package.bzl)
* **SDK-Based Rule**: [fuchsia_prebuilt_package.bzl](//build/bazel_sdk/bazel_rules_fuchsia/fuchsia/private/fuchsia_prebuilt_package.bzl)
* **Non-SDK In-Tree Rule**: [prebuilt_package.bzl](//build/bazel/rules/packages/prebuilt_package.bzl)

---

### Example B: Product Assembly Configuration (`product_configuration`)
This rule/macro manages complex JSON serialization and config validation.
* **Common Providers**: [providers.bzl](//build/bazel_sdk/fuchsia_rules_common/assembly/providers.bzl)
* **Common Implementation**: [product_configuration.bzl](//build/bazel_sdk/fuchsia_rules_common/assembly/product_configuration.bzl)
* **SDK-Based Rule**: [fuchsia_product_configuration.bzl](//build/bazel_sdk/bazel_rules_fuchsia/fuchsia/private/assembly/fuchsia_product_configuration.bzl)
* **Non-SDK In-Tree Rule**: [product_configuration.bzl](//build/bazel/rules/assembly/product_configuration.bzl)
