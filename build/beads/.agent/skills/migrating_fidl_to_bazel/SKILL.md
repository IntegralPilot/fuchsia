---
name: migrating-fidl-to-bazel
description: >-
  Migrates FIDL library targets from the GN build system to Bazel. Use when a
  user asks to migrate a FIDL target under //sdk/fidl to Bazel, create a
  BUILD.bazel file for a FIDL library, or sync a GN FIDL target to Bazel.
---

# Migrating FIDL from GN to Bazel

This skill guides the migration of FIDL library targets under //sdk/fidl from
GN to Bazel.

## 1. Create the Bazel target

1.  Identify the requested `BUILD.gn` files and their `fidl` GN targets based
    on the user request.
2.  Create a `BUILD.bazel` file in the same directory as the `BUILD.gn` file.
    See `assets/copyright_header_template.md` for the copyright header template.
    Add the copyright header to the top of the file.

3.  Define the equivalent Bazel target for the FIDL library. Use `fidl_library`
    loaded from `//build/bazel/rules/fidl:fidl_library.bzl`.

    > [!IMPORTANT]
    > **Preserve Comments:** Except the copyright header, you MUST copy all
    comments from the `BUILD.gn` file to the `BUILD.bazel` file. If a comment
    is above or on the same line as an attribute in `BUILD.gn`, it should be
    placed in the same relative position to the mapped attribute in `BUILD.bazel`.
    *(Example: If a comment is above `excluded_checks = [`, it should sit directly
    above `excluded_checks = [` in the `BUILD.bazel` file).*

    Map the attributes according to the attributes mapping in `references/gn_to_bazel_attributes_mapping.md`

4.  If dependencies are missing Bazel targets, migrate those dependencies first.
5.  Verify the Bazel target builds successfully:

    ```bash
    fx bazel build --config=fuchsia_platform //sdk/fidl/{library_name}:{library_name}
    ```

## 2. Register the target

1.  If the newly created Bazel target has a `category` attribute, add its IDK atom target to the appropriate list in `//sdk/fidl/category_lists.bzl` based on the rules in `references/idk_atom_target_registration_rules.md`.
2. If the migrated libraries are under `//sdk/fidl` directory, add the label of the verification target
   `//path/to/dir:verify_bazel2gn` to the `fidl_bazel2gn_verification_targets` list in `//sdk/fidl/bazel2gn_verification_targets.gni`, else add to the
   `bazel2gn_verification_targets` list in `//build/bazel2gn_verification_targets.gni`.

## 3. Sync FIDL targets back to GN

1.  Remove the `fidl(...)` target and the `import("//build/fidl/fidl.gni")`
    statement from the `BUILD.gn` file.
2.  Sync the target back from Bazel to GN using the `syncing-bazel-to-gn` skill
    (see `../syncing_bazel_to_gn/SKILL.md`).

## 4. Review, Verification and Cleanup

1.  Check the changes according to the checklist in
    `references/migration_code_review_checklist.md`.

2.  Format your changes:

    ```bash
    fx format-code --parallel
    ```

3.  Verify the build executes correctly for compatibility tests:

    ```bash
    fx set core.x64
    fx build //sdk/fidl:compatibility_tests
    fx bazel build --config=fuchsia_platform //sdk/fidl:compatibility_tests
    ```

4.  If the migrated FIDL library does NOT have the `category` attribute set, you
    MUST run a full build to verify:

    ```bash
    fx build
    ```

5.  Address and fix any build errors that occur during verification.
