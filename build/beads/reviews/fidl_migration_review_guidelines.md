# FIDL to Bazel Migration Review Guidelines

This document outlines the criteria and standards for validating changes that migrate FIDL libraries from GN to Bazel.

## 1. File Structure

- **Copyright Standards:** The standard Fuchsia copyright block on newly created `BUILD.bazel` files must use the current year, or the previous year if the file was originally uploaded then.
- **Consistent Target Naming:** The target name defined in the new `BUILD.bazel` file must exactly match the legacy target name in `BUILD.gn`.

## 2. Attribute Parity

Full parity must be observed between the GN and Bazel configurations:

- **Full Mapping:**
  - All attributes in `BUILD.gn`, except for `api = "{api_name}"` (where `{api_name}` equals to the target name plus `.api` suffix), must be mapped to attributes in `BUILD.bazel`.
- **Value Equality:** Attributes mapped across both `BUILD.gn` and `BUILD.bazel` must have the same values. For added `visibility`, they must be equivalent but not identical.
- **No Extra Attributes:** Extra attributes, except for `visibility`, should not be added to `BUILD.bazel` that are not present in `BUILD.gn`.
- **Comment Preservation:** Existing comments and inline `TODO` trackers must be cleanly carried over to the corresponding line in `BUILD.bazel`.

## 3. Rule Imports

- **Bazel Rule Definitions:** In `BUILD.bazel`, the standard `fidl_library` rule should be loaded from `//build/bazel/rules/fidl:fidl_library.bzl`:

```bazel
load("//build/bazel/rules/fidl:fidl_library.bzl", "fidl_library")
```

## 4. Graph Verification

- **Build Verifications (`//sdk/fidl/bazel2gn_verification_targets.gni`):** Newly migrated library targets must be added to the `fidl_bazel2gn_verification_targets` list inside `//sdk/fidl/bazel2gn_verification_targets.gni`.
- **SDK Categorization (`//sdk/fidl/category_lists.bzl`):** The corresponding IDK atom target (`{target_name}_idk`) must be added to the correct target list in `//sdk/fidl/category_lists.bzl`, based strictly on its assigned `category` and `stable` properties.

## 5. Code Standards & Git Messages

- **Code Formatter:** `BUILD.gn` and `BUILD.bazel` files must pass buildifier lint checks (`fx format-code`).
- **Commit Subject Line:**
  - Use consistent tagging `[bazel_migration]`.
- **Bug Identifiers:** Format tracking bug as `Bug: <issue-id>` in the footer.
