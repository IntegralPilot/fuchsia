# sdk-tools - Temporary switchback for SDK based package management.

These tools are used in the Fuchsia SDK for publishing and serving packages.
They are wrappers around `pm` and `ssh` that make sure the paths and options
used by each of these tools are consistent to reduce the typing needed by
developers and reduce developer friction.

These tools should be seen as a short term solution between now and when `ffx`
package and target management functions are complete.

Developer workflows or processes should not depend on these tools without first
coordinating with the OWNERS.

## Fuchsia Build Configuration

To build these tools, add them to your Fuchsia build configuration:

```bash
fx set <product>.<arch> --with //tools/sdk-tools:tools
```

## Running Tests

To run the unit tests associated with the SDK tools, use:

```bash
fx test //tools/sdk-tools:tests
```

Note: The `fx test` command has been verified as matching the targets defined in
[BUILD.gn](file:///usr/local/google/home/kyol/fuchsia/tools/sdk-tools/BUILD.gn)
but may not be run locally if there are active documentation or infrastructure
failures in the checkout.

## Removing a Tool from sdk-tools

When deprecating and removing an SDK tool from `//tools/sdk-tools/` (for
example, when its functionality has been fully integrated into `ffx`), you
must clean up its source code, build targets, SDK definitions, Bazel rules,
and documentation tools.

Important: Before beginning the deprecation and removal of any tool, you
must check all downstream consumers of the tool (both internal and external)
to ensure that they have successfully migrated off of it.

Note: Not all tools in `//tools/sdk-tools/` are integrated into the SDK or Bazel
toolchains in the same way. The checklist below is a comprehensive list based
on a full-featured SDK tool (like `fssh`). Some tools may not implement or use
all of these integrations, so certain steps may be skipped if they do not
apply to the tool being removed.

Below is a checklist of the files and configurations that typically need to be
updated:

1. **Delete the Tool Source Code**:

   * Delete the tool's subdirectory (e.g., `//tools/sdk-tools/<tool_name>/`).
   * Delete any associated integration or E2E tests that are specific to the
     tool (e.g., `//tools/sdk-tools/<tool_name>_integration/`).

2. **Update the Parent Build file (`//tools/sdk-tools/BUILD.gn`)**:

   * Remove the tool's host target from `group("tools")` `public_deps`.
   * Remove the tool's tests from `group("tests")` `deps`.

3. **Remove from SDK Molecules and Manifests**:

   * In `//sdk/BUILD.gn`, remove the tool's SDK target (e.g.,
     `//tools/sdk-tools/<tool_name>:<tool_name>_sdk`) from
     `sdk_molecule("host_tools")` or whichever molecule exports it.
   * In `//sdk/manifests/pdk.manifest` (and any other relevant SDK manifests),
     remove the host tool entries for each architecture (e.g.,
     `host_tool://tools/arm64/<tool_name>` and
     `host_tool://tools/x64/<tool_name>`).

4. **Update Bazel SDK Rules and Templates**:

   * In
     `//build/bazel_sdk/bazel_rules_fuchsia/fuchsia/workspace/fuchsia_toolchain_info.bzl`:
     * Remove the tool attribute from `_fuchsia_toolchain_info_impl` function.
     * Remove the tool's `attr.label` attribute from the
       `fuchsia_toolchain_info` rule definition.
   * In
     `//build/bazel_sdk/bazel_rules_fuchsia/fuchsia/workspace/sdk_templates/fuchsia_sdk.BUILD.bazel`:
     * Remove the tool parameter from the `fuchsia_toolchain_info(...)`
       instantiation.
     * Remove the corresponding `sdk_host_tool(name = "<tool_name>")` target.

5. **Update CLI Documentation Tooling**:

   * In `//tools/clidoc/src/main.rs`:
     * Remove the tool's name from the `ALLOW_LIST`.
     * Remove the tool's name from `IGNORE_ERR_CODE` (if present).
   * In `//tools/docsgen/clidoc_test.py`:
     * Remove the tool's expected markdown file (e.g., `clidoc/<tool_name>.md`)
       from `WANT_NAMES`.

For a complete example of a tool removal change, see Gerrit change
[1628402](https://fuchsia-review.googlesource.com/c/fuchsia/+/1628402), which
removed `fssh` and `fstar_integration` from the Fuchsia codebase.
