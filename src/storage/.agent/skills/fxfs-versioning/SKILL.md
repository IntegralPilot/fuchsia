---
name: fxfs-versioning
description: Updating Fxfs versioned structures, cascading changes, type fingerprints, and golden images.
---

# Fxfs Versioning

Fxfs manages on-disk format compatibility using a project-wide `LATEST_VERSION`. Use this skill when implementing structural changes or storage format updates.

## Structural Changes (Major Version Bump)

When modifying a versioned structure, follow this standard procedure (the "versioning dance"):

1.  **Bump `LATEST_VERSION`**: Update the `LATEST_VERSION` constant in [types.rs](src/storage/fxfs/src/serialized_types/types.rs).
2.  **Version the Structure**:
    - Copy the existing `FooV(N)` to `FooV(LATEST_VERSION)` (e.g., `FooV53` to `FooV55`).
    - **Clean the Legacy Type**: Remove extraneous traits (`Debug`, `Eq`, `PartialEq`, `TypeFingerprint`, `Versioned`) from the older version (`FooV(N)`).
    - **Preserve Documentation on the Latest Version**: Ensure that `FooV(LATEST_VERSION)` retains all documentation comments (`///`) on the struct and its fields/variants.
    - **Strip Documentation from Legacy Version**: As per convention, explicitly strip **all** documentation comments (`///`) from the older struct `FooV(N)` and its fields/variants. Only the latest version should retain documentation.
    - **Move Legacy Types to `legacy` Submodules**: If the module has a dedicated `legacy` submodule (such as `object_record/legacy.rs`), move older versioned structs (`FooV(N)`) and their `From` implementations into that submodule so the main module stays clean and contains only the active `LATEST_VERSION` definitions. **Important**: Always include `#[cfg_attr(fuzz, derive(arbitrary::Arbitrary))]` on structs and enums moved to `legacy` submodules if they are reachable from fuzzed types (like `ObjectValueV(N)`), otherwise fuzzer builds (`--cfg=fuzz`) will fail with `Arbitrary` trait derivation errors.
    - **Visibility**: Remove `pub` from the legacy type's fields and from the type itself in almost all cases. Nothing should use them except for the deserialization `into()` chains (which are defined in the same module) and other legacy version structs.
    - **Methods**: Methods should **always** remain on the unversioned type alias (e.g., `impl Foo { ... }`). Do not leave methods on the legacy struct (e.g., `impl FooV(N) { ... }`) unless a method is explicitly ONLY used for performing the data migration itself.
    - **Use Unversioned Aliases**: Never use versioned typenames (e.g., `ObjectKeyV54`) outside of type or struct definitions. The philosophy is to remain unintrusive to normal code. Functional code and general tests should **always** use the unversioned alias (e.g., `ObjectKey`). Only explicitly use versioned names in tests when specifically testing a versioning-related scenario. However, you must ALWAYS use versioned types when defining other structs to prevent the internal layout from fundamentally changing underneath you.
    - **Item Aggregations (`ObjectItem` / `Item` / `ItemRef`)**: When versioning aggregations that compose separate structs like `Item<K, V>`, ensure you define the legacy item tuple correctly using the corresponding legacy component types (e.g., `pub(crate) type ObjectItemV50 = Item<ObjectKeyV43, ObjectValueV50>;`).

3.  **Implement Migration**:
    - **Prefer the `#[derive(Migrate)]` macro** over an explicit `impl From` wherever possible. For structurally identical types, it generates the `From<FooV(N)> for FooV(LATEST_VERSION)` boilerplate automatically by using `.into()` recursively on child structures.
    - **Avoid Trait Conflicts (`E0119`)**: Do NOT write a manual `impl From<Old> for New` if the `Old` structure is annotated with `#[derive(Migrate)]` and `#[migrate_to_version(New)]` unless its variants drastically differ. Wait to see if the macro handles it first.
    - **The `#[migrate_nodefault]` Attribute**: If `Old` has identical field names/variants to `New` but you do *not* want the `Migrate` macro to automatically use `Default::default()` to fill in any missing or mismatched fields, you **must** apply `#[migrate_nodefault]` to the `Old` struct/enum. This ensures exact mapping and avoids confusing `Default is not implemented` errors.
    - When the macro is truly insufficient (e.g., dropping fields or fundamentally altering semantics), drop `#[derive(Migrate)]` and manually define `impl From<FooV(N)> for FooV(LATEST_VERSION)`.

4.  **Handle Cascading Updates**:
    - If a parent structure `Bar` contains `Foo`, then `Bar` must also be versioned (e.g., to `BarV55`) to reference the new `FooV55`.
    - Apply this same process recursively up the structure hierarchy.
    - **Rust 2024 Implicit Borrowing Rules**: When updating match patterns (e.g., `let ObjectValue::Object { ... } = &mut mutation.value`), do **not** use `ref mut` bindings on internal fields. The compiler enforces implicit borrowing on reference patterns, and using `ref mut` will result in `cannot explicitly borrow within an implicitly-borrowing pattern` errors.
5.  **Update Type Mappings**:
    - Add the new version mapping to the `versioned_types!` macro in [types.rs](src/storage/fxfs/src/serialized_types/types.rs).
6.  **Verify via Type Fingerprints**:
    - Update the `TypeFingerprint` implementation for the latest version.
    - Create a new type fingerprint test case in `tests.rs` to validate the new version.
    - When bumping the version, you must add a new golden file for type fingerprints (`src/serialized_types/golden/NN.json.golden`).
    - Ensure `//src/storage/fxfs:type_fingerprints_golden_test` is in your GN graph (`fx add-test //src/storage/fxfs:type_fingerprints_golden_test`) so it runs locally.
    - The easiest way to generate this is to run `fx build`. It will fail with a "Golden file mismatch" and output an `fx run-in-build-dir cp ...` command. Run that command to copy the generated `.json.golden` into your source tree, then `git add` it. Alternatively, build with `fx set ... --args update_goldens=true`.
7.  **Generate Golden Images (.zstd)**:
    - Regenerate the on-disk image test suite by running: `fx fxfs create_golden`.
    - This creates a new `fxfs_golden.NN.0.img.zstd` file in `testdata/`. Ensure you `git add` this new untracked file, along with adding it to `testdata/images.gni`.
    - This ensures backwards compatibility and is required for CQ verification.

## Pruning Legacy Versions

Removing old versioned code is a destructive operation that breaks compatibility with older devices.

> [!CAUTION]
> **Breaking Change**: Deleting older versions prevents Fxfs from mounting filesystems created with those versions. This is an infrequent operation that requires official project coordination.

1.  **Update Support Window**: Raise `EARLIEST_SUPPORTED_VERSION` in [types.rs](src/storage/fxfs/src/serialized_types/types.rs).
2.  **Clean Codebase**: Remove legacy struct definitions and their associated `From` conversion traits.
3.  **Refresh Test Assets**:
    - Delete the obsolete `.zstd` golden image in `testdata/`.
    - Delete the obsolete `.json.golden` in `src/serialized_types/golden/`.
    - Remove the corresponding entry from `images.gni`.
4.  **Finalize**: Run `fx fxfs create_golden` to update the remaining images and fingerprints.
