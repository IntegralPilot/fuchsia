---
name: syncing-bazel-to-gn
description: Syncing Bazel targets to GN with bazel2gn
---

# Syncing Bazel Targets to GN

This process is necessary if you've migrated a target that is still referenced
by other GN targets, so you can't delete the migrated GN target immediately.

Common use cases include migrating a **library** target that is still referenced
by other binary or test targets in GN.

**NOTE:** Host tool targets are binary targets, not library targets. It is very
rare that you need to automatically sync host tool targets to GN with
`bazel2gn`.

## Steps

Use [`bazel2gn`](../../../../../tools/bazel2gn/README.md) to automatically sync
the Bazel targets to GN:

1.  **Sync targets:**

    Run the following command to sync targets defined in
    `path/to/dir/BUILD.bazel` to `path/to/dir/BUILD.gn`:

    ```bash
    fx bazel2gn -d path/to/dir
    ```

2.  **Clean up GN:**

    Remove the old GN targets you've migrated from `path/to/dir/BUILD.gn`.

3.  **Add verification:**

    - For targets under `//sdk/fidl`, add
      `//sdk/fidl/fuchsia.some.fidl:verify_bazel2gn` to the
      `fidl_bazel2gn_verification_targets` list in
      `//sdk/fidl/bazel2gn_verification_targets.gni`.
    - For other targets, add
      `//path/to/dir:verify_bazel2gn` to the `bazel2gn_verification_targets`
      list in `//build/bazel2gn_verification_targets.gni`.

4.  **Verify:**

    Confirm your target sync is successful by running:

    ```bash
    fx build --host //build:bazel2gn_verifications
    ```

## Third-Party Dependency Translation

`bazel2gn` automatically translates Bazel external repository targets (such as
`@com_google_googletest//:gtest`, `@re2//:re2`, `@boringssl//:crypto`) to their
equivalent GN labels according to [`//build/tools/bazel2gn/third_party_target_map.json`](../../../../../tools/bazel2gn/third_party_target_map.json).

If an external repository dependency (starting with `@`) is not in this mapping,
`bazel2gn` will print a warning to `stderr` and append an inline
`# BAZEL2GN_WARNING: Unknown Bazel repository name` comment to the generated GN line.
