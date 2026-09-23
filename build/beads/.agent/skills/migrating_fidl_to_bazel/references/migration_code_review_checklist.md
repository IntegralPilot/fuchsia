---
name: GN-to-Bazel-Migration-Code-Review-Checklist
description: >
  This is a checklist for code reviews of GN to Bazel migrations.
---

# GN to Bazel Migration Code Review Checklist

## General

* [ ] The copyright year in the new generated BUILD.bazel file is the current year.
* [ ] The copyright header in the BUILD.gn file is not changed after the migration.
* [ ] The comment above or at the same line with the attribute in the BUILD.gn file before migration is copied to the same position related to the mapped attribute in the new generated BUILD.bazel file.
* [ ] The fidl_library is loaded from `//build/bazel/rules/fidl:fidl_library.bzl`.
* [ ] Except those in exception_list, all the attributes in the BUILD.gn are migrated to the BUILD.bazel file according to the attributes mapping in `references/gn_to_bazel_attributes_mapping.md`.
  The exception_list is as follows:
  - `api`
* [ ] The `fidl` target is included in the correct list in `//sdk/fidl/category_lists.bzl` based on its category according to the rules in `references/idk_atom_target_registration_rules.md`.
* [ ] For FIDL libraries under `//sdk/fidl` directory, the label of the verification target `//path/to/dir:verify_bazel2gn` is added to the
    `fidl_bazel2gn_verification_targets` list in `//sdk/fidl/bazel2gn_verification_targets.gni`.
* [ ] For FIDL libraries outside `//sdk/fidl` directory, the label of the verification target `//path/to/dir:verify_bazel2gn` is added to the
    `bazel2gn_verification_targets` list in `//build/bazel2gn_verification_targets.gni`.