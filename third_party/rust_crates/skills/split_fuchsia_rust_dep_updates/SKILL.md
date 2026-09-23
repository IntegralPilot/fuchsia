---
name: split-fuchsia-rust-dep-updates
description: >-
  Splits a Fuchsia Rust dependency update commit into two or more commits: one or
  more commits with pure copies/moves of vendored crates, and one final commit with
  actual changes and migrations. Similar to Gerrit review splitting. Use when
  preparing a Rust dependency upgrade CL in Fuchsia where vendored crates are
  added, deleted, or updated, to make the diff reviewable.
---

# Splitting Fuchsia Rust Dependency Updates

Splits a dependency update commit into two parts (or multiple grouped commits):
1.  **Pure copies/moves**: Git sees them as 100% similar, making the diff clean.
1.  **Actual changes**: Contains content updates, Cargo.toml/Cargo.lock updates, and code migrations.

This structure makes it much easier for reviewers in Gerrit to see what actually changed in the dependency upgrade, rather than being overwhelmed by a huge diff of new vendored code.

> **Note**: For the end-to-end workflow on planning dependency updates, verifying compilation/lints (`fx clippy --all`), and managing multi-repository graceful migrations (`//vendor/...`), see the top-level **[`update-fuchsia-rust-deps`](third_party/rust_crates/skills/update_fuchsia_rust_deps/SKILL.md)** skill. This skill (`split-fuchsia-rust-dep-updates`) focuses specifically on structuring your commits for clean Gerrit code reviews.

## Workflow Overview

1.  Identify which crates are completely upgraded (moves) and which have multiple versions kept (copies).
1.  Run either `split_updates.py` (for a single copy/move commit) or `split_updates_grouped.py` (for sequential grouped copy/move commits) to automate the split.
1.  Verify the split was successful.

---

## 1. Automated Splitting

### Script Architecture

The splitting scripts share a common helper module (`split_utils.py`):
*   **`split_utils.py`**: Shared utility library providing repository validation, revision resolution, vendored crate discovery (`git ls-tree`), copy/move classification (`discover_copies_and_moves`), filesystem/git operations (`apply_copies`, `apply_moves`), and flexible grouping strategies (`group_operations`).
*   **`split_updates.py`**: CLI script for splitting an upgrade into a single copy/move commit followed by the actual updates commit.
*   **`split_updates_grouped.py`**: General-purpose CLI script for partitioning copies and moves across multiple sequential commit groups before the actual updates commit.

---

### Single Copy/Move Commit (`split_updates.py`)

Use `split_updates.py` to automate splitting a dependency upgrade into exactly two commits:
1. One commit with all pure copies and moves under `third_party/rust_crates/vendor/`.
1. One commit with the actual updates and migrations.

Run the script from the root of your Fuchsia repository:

```bash
python3 third_party/rust_crates/skills/split_fuchsia_rust_dep_updates/scripts/split_updates.py \
  --repo-path=$(git rev-parse --show-toplevel) \
  --target-commit={commit_or_branch_to_split} \
  --base-commit={base_commit_usually_origin_main}
```

#### Options

Flag | Default | Description
--- | --- | ---
`--repo-path` | (Required) | Absolute path to the local Fuchsia repository. Use `$(git rev-parse --show-toplevel)` to automatically detect it.
`--target-commit` | `HEAD` | The commit (or branch) containing the combined updates and migrations that you want to split.
`--base-commit` | `origin/main` | The base commit before the updates were applied.

#### Success Criteria

Upon success, the script will output two commit hashes:
*   **Commit 1 (copies/moves)**: Contains only directory moves and copies under `third_party/rust_crates/vendor/`.
*   **Commit 2 (actual updates)**: Contains the rest of the changes.

You will be left in a detached HEAD state at Commit 2. Update your branch to this state:

```bash
git checkout -B {your_branch_name}
```

---

### Grouped Copy/Move Commits (`split_updates_grouped.py`)

For large dependency updates involving many crates, a single copy/move commit can still be difficult to navigate in Gerrit. `split_updates_grouped.py` dynamically discovers all crate copy/move operations and chunks them into multiple smaller sequential commits before the final actual update commit.

```bash
python3 third_party/rust_crates/skills/split_fuchsia_rust_dep_updates/scripts/split_updates_grouped.py \
  --repo-path=$(git rev-parse --show-toplevel) \
  --target-commit={commit_or_branch_to_split} \
  --base-commit={base_commit_usually_origin_main} \
  --crates-per-group=10
```

#### Options

Flag | Default | Description
--- | --- | ---
`--repo-path` | (Required) | Absolute path to the local Fuchsia repository.
`--target-commit` | `HEAD` | The commit (or branch) containing the combined updates to split.
`--base-commit` | `origin/main` | The base commit before the updates were applied.
`--crates-per-group` / `--batch-size` | `10` | Automatically chunks crate copy/move operations into sequential commits of at most N crates per commit (used if `--groups-file` and `--num-groups` are not specified).
`--num-groups` | `None` | Divides discovered crate copy/move operations evenly across K sequential commit groups.
`--groups-file` | `None` | Path to a JSON file specifying explicit custom crate groupings.

*Note on Option Precedence*: If multiple grouping options are supplied, `--groups-file` takes highest precedence, followed by `--num-groups`, and finally `--crates-per-group` / `--batch-size`.

#### Grouping Examples

1.  **Chunk by batch size (e.g., 15 crates per commit)**:
    ```bash
    python3 third_party/rust_crates/skills/split_fuchsia_rust_dep_updates/scripts/split_updates_grouped.py \
      --repo-path=$(git rev-parse --show-toplevel) \
      --crates-per-group=15
    ```

1.  **Divide evenly into K commit groups**:
    ```bash
    python3 third_party/rust_crates/skills/split_fuchsia_rust_dep_updates/scripts/split_updates_grouped.py \
      --repo-path=$(git rev-parse --show-toplevel) \
      --num-groups=4
    ```

1.  **Custom JSON grouping file**:
    Create a JSON file (e.g., `groups.json`) defining lists of crate names, either as a list of lists:
    ```json
    [
      ["aes", "block-buffer", "cbc"],
      ["digest", "sha2", "hmac"]
    ]
    ```
    or as a dictionary mapping group names to crate arrays:
    ```json
    {
      "ciphers": ["aes", "block-buffer", "cbc"],
      "hashes": ["digest", "sha2", "hmac"]
    }
    ```
    Then pass it to the script:
    ```bash
    python3 third_party/rust_crates/skills/split_fuchsia_rust_dep_updates/scripts/split_updates_grouped.py \
      --repo-path=$(git rev-parse --show-toplevel) \
      --groups-file=groups.json
    ```

---

## 2. Verification

### Verification of Pure Copies & Moves

Gerrit does not always automatically detect when a vendored crate directory is copied or moved. To verify locally that a copy/move commit contains **zero content changes** to the copied or moved files, run the following verification commands:

#### 1. Tree-Wide Similarity Verification
Verify that every file added or modified in the copy/move commit is a 100% identical copy (`C100`) or 100% rename (`R100`):

```bash
git diff --raw -M100% -C100% --find-copies-harder {base_commit} {copy_move_commit}
```

Every modified file entry should show `C100` or `R100` with 100% similarity.

#### 2. Per-Crate Directory Verification
For each copied or moved crate, compare the source directory at `{base_commit}` against the target directory at `{copy_move_commit}` using git tree diffing:

```bash
git diff {base_commit}:third_party/rust_crates/vendor/{old_crate}-{old_version} {copy_move_commit}:third_party/rust_crates/vendor/{new_crate}-{new_version}
```

This command **must produce empty output** (exit status 0) for every copied or moved crate, proving that no content modifications were made during the copy or move.

#### 3. Listing Verification Commands in Commit Messages
Always list these explicit per-crate `git diff` commands in the body of the `[rust-3p] Copy and move vendored crates` commit message so that reviewers in Gerrit can easily copy-paste and verify the 100% purity of the copy/move commit locally.

---

### Tree Identity Check

Verify that the split did not introduce any changes compared to the original target state. The diff between your new HEAD (the final actual updates commit) and the original target commit must be empty:

```bash
git diff {original_target_commit} HEAD
```

If the diff is empty, the split was successful and the tree state is identical to your original target commit.

> **Note**: For building, running `fx clippy --all`, and verifying host-only targets, follow the verification steps in the **`third_party/rust_crates/skills/update_fuchsia_rust_deps`** skill.
---

## 3. Manual Steps (Fallback)

If the script fails (e.g., due to cherry-pick conflicts), you can perform the steps manually:

1.  **Identify copies and moves**:
    *   Compare `origin/main` (base) and `HEAD` (target).
    *   If both old and new versions exist in target: it is a **copy** (e.g. `aes-0.8.4` -> `aes-0.9.1`).
    *   If only the new version exists in target: it is a **move** (e.g. `chacha20-0.9.1` -> `chacha20-0.10.0`).

1.  **Checkout base**:
    ```bash
    git checkout origin/main
    ```

1.  **Apply copies**:
    For each copy `old` -> `new`:
    ```bash
    rm -rf third_party/rust_crates/vendor/{new}
    cp -r third_party/rust_crates/vendor/{old} third_party/rust_crates/vendor/{new}
    git add third_party/rust_crates/vendor/{new}
    ```

1.  **Apply moves**:
    For each move `old` -> `new`:
    ```bash
    git checkout origin/main -- third_party/rust_crates/vendor/{old}
    rm -rf third_party/rust_crates/vendor/{new}
    git add third_party/rust_crates/vendor/{old}
    git mv third_party/rust_crates/vendor/{old} third_party/rust_crates/vendor/{new}
    ```

1.  **Commit copies/moves**:
    ```bash
    git commit -m "[rust-3p] Copy and move vendored crates"
    ```

1.  **Restore base tree temporarily**:
    Create a temporary commit restoring the base commit's exact tree so that cherry-picking applies cleanly without tree conflicts:
    ```bash
    git reset --hard $(git commit-tree $(git rev-parse origin/main^{tree}) -p HEAD -m "[rust-3p] Temporary restore base tree")
    ```

1.  **Cherry-pick target**:
    ```bash
    git cherry-pick {target_commit}
    ```
    Resolve any conflicts if they occur.

1.  **Finalize actual updates commit**:
    Reset softly back to your copy/move commit and commit with the target commit's message and authorship:
    ```bash
    git reset --soft HEAD~2
    git commit -C {target_commit}
    ```
