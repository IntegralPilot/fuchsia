---
name: update-fuchsia-rust-deps
description: >-
  Comprehensive workflow for updating third-party Rust crate dependencies in Fuchsia,
  verifying updates with clippy and build/tests, managing multi-repository graceful
  migrations, and preparing review-friendly CLs.
---

# Updating Fuchsia Rust Dependencies

This top-level skill describes the end-to-end workflow for upgrading third-party Rust crate dependencies under `third_party/rust_crates/`, verifying that the tree compiles and lints cleanly, gracefully handling out-of-tree or multi-repository dependents, and structuring commits for Gerrit code review.

---

## 1. Planning & Performing the Update

### Separate into Independent Stacks
Before starting a large dependency upgrade, check if the updates can be broken into **smaller, independent stacks** that can be developed, compiled, and landed in parallel.

For example, when updating a suite of cryptographic crates:
*   **Block Cipher Stack** (`cipher`, `aes`, `chacha20`, `cbc`) is independent of digests.
*   **Digest Stack** (`digest`, `sha2`, `hmac`, `hkdf`) depends on block ciphers but is independent of signatures.
*   **ECC Stack** (`p256`, `elliptic-curve`, `ecdsa`) might depend on digest versions already in tree.

Upgrading separate stacks on distinct branches reduces risk and keeps Change Lists (CLs) manageable.

### Editing `Cargo.toml` & Regenerating Targets
1.  Update crate version requirements or add legacy package aliases in `third_party/rust_crates/Cargo.toml`.
1.  Run `fx update-rustc-third-party` to regenerate `Cargo.lock`, vendored crates under `third_party/rust_crates/vendor/`, and `BUILD.gn` targets:
    ```bash
    fx update-rustc-third-party
    ```

---

## 2. Verifying the Update

### 1. Fast Compilation & Lint Check (`fx clippy --all`)
Always run `fx clippy --all` first before running a full build. Clippy type-checks and lints across all Rust targets significantly faster than a full build:

```bash
./scripts/fx clippy --all
```

If compilation or lint errors occur, inspect the diagnostics and update affected in-tree Fuchsia targets to match the new crate APIs.

### 2. Full Build (`fx build`)
After `fx clippy --all` passes cleanly, run a full build to verify non-Rust targets, linking, and C/C++ FFI bindings:

```bash
./scripts/fx build
```

### 3. Verifying Host-Only Targets
Some host-only components (such as `partitions_config`) may not be part of the default target-toolchain build graph:
1.  **Build with Host Toolchain**: Explicitly specify the host toolchain label:
    ```bash
    ./scripts/fx build "//src/lib/assembly/partitions_config:partitions_config(//build/toolchain:host_x64)"
    ```

---

## 3. Multi-Repository Migration Workflow

When a dependency upgrade affects code in another repository (such as `//vendor/google` or other `jiri`-managed sub-repositories), we can update `fuchsia.git` and `vendor/google` at the same time using cross-repository `Depends-On` footers in Gerrit.

### 1. Simultaneous Cross-Repository Updates (`Depends-On`)

You can update `fuchsia.git` and `vendor/google` simultaneously without needing sequential staged releases:

1.  **Create `fuchsia.git` Commit**: Apply the crate upgrade, `Cargo.toml`/`Cargo.lock` changes, and in-tree `fuchsia.git` migrations.
2.  **Create `vendor/google` Commit**: Update code and `BUILD.gn` dependencies in `vendor/google` to match the new crate APIs.
3.  **Link Commits with `Depends-On` Footers**:
    *   In the **`fuchsia.git`** commit message, add a `Depends-On` footer pointing to the `vendor/google` (`turquoise-internal`) Change-Id:
        ```
        Depends-On: turquoise-internal:$Commit-Id
        ```
    *   In the **`vendor/google`** commit message, add a `Depends-On` footer pointing to the `fuchsia.git` Change-Id:
        ```
        Depends-On: fuchsia:$Commit-Id
        ```
4.  **Land via Fuchsia CQ**: Gerrit and Fuchsia CQ will test both CLs together in a single integration run and land them atomically across repositories.

---

### 2. Sequential Staged Rollout Workflow (Alternative)

If a simultaneous update is not possible and out-of-tree references must be temporarily pinned:

1.  **Check Sibling Dependencies & Transitive Stacks**: First verify whether updating sibling dependencies together allows `fuchsia.git` targets to compile cleanly.
    *   **Shared Foundational Crates (e.g., `der`, `spki`)**: When a target in a sub-repository depends on both an upgraded crate (e.g., `pkcs8 0.11.0`) and sibling crates (`sec1`, `pkcs1`) that share foundational types (`der`, `spki`), mixing versions across those crates will cause trait bound errors (`Decode`, `Encode`). Ensure either:
        *   The sub-repository remains on the legacy versioned target (`//third_party/rust_crates:pkcs8-0.10.2`) until all sibling cryptographic crates in that component can be migrated to the new foundational stack together, OR
        *   When migrating off the legacy pin, update all top-level foundational/sibling crates in `Cargo.toml` (`der`, `spki`) so the entire stack shares consistent types.
2.  **Define Structured GN Groups Before Pinning Sub-Repositories**: Before updating sub-repository build files (`//vendor/...`) to pin to a legacy crate version, first land a preparatory patch in `fuchsia.git` that sets up structured `groups` on the existing crate version under `[gn.package.<PackageName>.<Version>]` in `third_party/rust_crates/Cargo.toml`. Because defining explicit `groups` on an active dependency overrides `cargo-gnaw`'s default group generation, explicitly list the default unversioned group (`{ name = "foo" }`), the major-version group (`{ name = "foo-0.3" }`), and the exact patch version group (`{ name = "foo-0.3.2" }`):
    ```toml
    [gn.package.foo."0.3.2"]
    groups = [
      { name = "foo" },
      { name = "foo-0.3" },
      { name = "foo-0.3.2" }
    ]
    ```
    Always depend on the major-version group (`//third_party/rust_crates:foo-0.3` or `//third_party/rust_crates:pkcs8-0.10`) in sub-repositories (`//vendor/...`) rather than patch-specific targets (`:foo-0.3.2`). Note: Do **not** depend directly on internal `rustc_library` targets (`:foo-v0_3_2`).
3.  **5-CL Sequential Rollout Workflow**:
    *   **Reference**: For guidance on managing these CLs, see the [split_fuchsia_rust_dep_updates](../split_fuchsia_rust_dep_updates/SKILL.md) skill.
    *   **CL 1 (`fuchsia.git`) - Define Legacy GN Groups**:
        *   Add the structured `groups` block above to `third_party/rust_crates/Cargo.toml` for the current active legacy version (`0.3.2`).
        *   Run `fx update-rustc-third-party` to emit `//third_party/rust_crates:foo-0.3` in `BUILD.gn`.
        *   Run `./scripts/fx clippy --all` and land CL 1 in `fuchsia.git`.
    *   **CL 2 (Sub-Repository e.g., `//vendor/...`) - Pin to Legacy Major-Version Group**:
        *   Now that `//third_party/rust_crates:foo-0.3` (`//third_party/rust_crates:pkcs8-0.10`) exists in tree, update the sub-repository's `BUILD.gn` to depend on it instead of `//third_party/rust_crates:foo`.
        *   Run `./scripts/fx clippy --all` to verify it passes cleanly.
        *   Check in the code within the sub-repository context (`git -C vendor/... commit`) using a commit message like:
            ```
            [rust-3p] Pin keymint to legacy pkcs8-0.10

            Pin libkmr_common to //third_party/rust_crates:pkcs8-0.10 prior to upgrading
            pkcs8 in fuchsia.git.

            Test: ./scripts/fx clippy --all
            ```
        *   Land CL 2 in the sub-repository.
    *   **CL 3 (`fuchsia.git`) - Upgrade Crate & Retain Legacy Alias**:
        *   Update `foo = "0.4.0"` in `third_party/rust_crates/Cargo.toml`.
        *   Add legacy alias `foo-0_3_2 = { package = "foo", version = "0.3.2" }`.
        *   Update `[gn.package.foo."0.3.2"]` to remove `{ name = "foo" }` so the upgraded `foo 0.4.0` owns `//third_party/rust_crates:foo`, while keeping `{ name = "foo-0.3" }` and `{ name = "foo-0.3.2" }`:
            ```toml
            [gn.package.foo."0.3.2"]
            groups = [
              { name = "foo-0.3" },
              { name = "foo-0.3.2" }
            ]
            ```
        *   Run `fx update-rustc-third-party` and `./scripts/fx clippy --all` across the tree, and commit CL 3 in `fuchsia.git`.
    *   **CL 4 (Sub-Repository e.g., `//vendor/...`) - Migrate to Upgraded Crate**:
        *   Update code in the sub-repository to use `foo 0.4.0` and switch its `BUILD.gn` dependency back to `//third_party/rust_crates:foo`.
        *   Run `./scripts/fx clippy --all` to verify compilation and linting.
        *   Check in the code within the sub-repository context (`git -C vendor/... commit`) using a commit message like:
            ```
            [rust-3p] Unpin keymint to use upgraded pkcs8

            Switch libkmr_common back to //third_party/rust_crates:pkcs8 now that
            all sibling dependencies have been upgraded.

            Test: ./scripts/fx clippy --all
            ```
        *   Land CL 4 in the sub-repository.
    *   **CL 5 (`fuchsia.git`) - Remove Legacy Crate**:
        *   Remove `foo-0_3_2` and its `[gn.package.foo."0.3.2"]` block from `third_party/rust_crates/Cargo.toml`.
        *   Run `fx update-rustc-third-party` and `./scripts/fx clippy --all` across the tree to confirm all targets compile cleanly without the legacy pin, and commit CL 5 in `fuchsia.git`.

---

## 4. Breaking Up Changes for Code Review

Dependency upgrades often introduce massive diffs due to added, removed, or updated vendored files under `third_party/rust_crates/vendor/`. To make CLs easy to review in Gerrit:

### Commit Message Formatting
When checking in the code, format the commit message clearly with a `[rust-3p]` prefix and bullet points listing every added, updated, or renamed crate and its version transition. For example:

```
[rust-3p] Update ecc crates and dependencies

This patch:

* Updates p256 from 0.11.1 to 0.13.2
* Renames p256 0.11.1 to p256-0_11
* Adds primeorder 0.13.6
* Updates rfc6979 from 0.3.1 to 0.4.0
* Updates serdect from 0.1.0 to 0.2.0
```

### Splitting Commits
*   **Delegate to `split-fuchsia-rust-dep-updates` Skill**: Refer to `third_party/rust_crates/skills/update_fuchsia_rust_deps/SKILL.md`
*   That skill automates splitting your update commit into:
    1.  **Commit 1 (Pure copies & moves)**: Git recognizes these as 100% similar, making the Gerrit diff clean.
    1.  **Commit 2 (Actual updates & API migrations)**: Contains `Cargo.toml`/`Cargo.lock` changes, vendored code modifications, and in-tree API migrations. Include the detailed `[rust-3p]` commit message listing the version updates here.
