---
name: chromium-media-merge
description: Complete instructions and workflow for incrementally merging upstream Chromium media commits into the Fuchsia chromium_media library. Use this skill when updating, merging, syncing, or resolving conflicts in chromium_media against upstream Chromium.
---

# Chromium Media Incremental Merge Workflow

This skill defines the rigorous, crash-resilient process for incrementally importing and merging upstream Chromium `media` commits into Fuchsia's `//src/media/third_party/chromium_media` library while preserving Fuchsia-specific customizations, build stability, and hardware decoder compatibility.

---

## MANDATORY FIRST STEP: Execute Prerequisites Check

Before executing any merge script, applying patches, or modifying files, you **MUST** read and execute the verification steps documented in [prerequisites.md](prerequisites.md).

In particular:
1. Verify that the upstream Chromium repository exists at the expected location (`local_src/chromium/src/media` or `CHROMIUM_REPO_MEDIA`).
2. If the Chromium checkout is missing or invalid, **STOP IMMEDIATELY** and ask the user to remedy this before continuing.
3. Ensure the Fuchsia workspace is clean or in an expected state.

---

## INITIALIZING `.merge_state/` (Only for Fresh Sync Sessions)

Check whether `src/media/third_party/chromium_media/.merge_state/todo_commits.txt` exists and contains pending commits.
- **If `.merge_state/todo_commits.txt` already exists**: Do **not** re-initialize. Skip directly to Section 2 (*The Incremental 3-Way Patch Workflow*).
- **If starting a fresh sync session** (where `.merge_state/` does not exist or needs re-initialization): Follow the procedure in [initialize_merge_state.md](initialize_merge_state.md). That guide explains how to read the starting commit hash from `README.fuchsia` (`Revision: ...`) and enumerate all upstream Chromium commits up to Chromium ToT that touch any of the maintained files.

---

## 1. Core Architecture & Scope

### Maintained Files Subset
Fuchsia maintains a curated subset of files in `chromium_media` (listed in [`scripts/maintained_files.txt`](scripts/maintained_files.txt)). Commits touching any of these files are processed in historical order; changes to unmaintained files are ignored.

### Intent of Merging Upstream Changes
The intent of the merge is to merge textually where possible, and to also consider whether we can incorporate the conceptual intent of the Chromium change, to the extent that it can apply to Fuchsia's usage of the code. Allowing for changes to Fuchsia call sites (in driver and codec code) is encouraged so that upstream improvements such as spanification, API modernizations, and safety improvements can be adopted rather than ignored when inconvenient.

### Core Customizations to Preserve During Conflict Resolution
When resolving conflicts, strictly preserve Fuchsia's local modifications:
- **Custom Headers & Build Tokens:** Include `#include "chromium_utils.h"` and Fuchsia-maintained helper headers (e.g., `media/video/bit_reader_macros.h`).
- **Standard Library Variations:** Use `cpp17::variant` / `cpp17::get` where Fuchsia's C++ standard library wrappers differ from upstream Chromium.
- **Decoder Heuristics & APIs:** Preserve Fuchsia-specific low-latency heuristics, custom NALU pre-parsing support, and explicit integer casts required by Fuchsia's `-Wimplicit-int-conversion` flags.

---

## 2. The Incremental 3-Way Patch Workflow

We use an automated hybrid patch workflow driven by [`scripts/apply_next_batch.py`](scripts/apply_next_batch.py):

1. **Path-Mapped Patch Application:**
   For each upstream SHA in `todo_commits.txt`, the script attempts to apply the patch directly to Fuchsia's directory structure:
   ```bash
   git apply --directory=src/media/third_party/chromium_media/media current_patch.diff
   ```
2. **Automatic 3-Way Merge Fallback (`git merge-file`):**
   When `git apply` is rejected due to Fuchsia local modifications, the script automatically invokes 3-way merge for each modified file:
   ```bash
   git merge-file -p <fuchsia_file> <base_file> <theirs_file>
   ```
   - Clean merges are applied automatically.
   - If conflict markers (`<<<<<<<` / `=======` / `>>>>>>>`) occur, execution **STOPS** for manual inspection.
3. **Manual Conflict Resolution & Summary Step:**
   - Inspect the conflicting file(s) and resolve markers according to the customization rules and merge intent in Section 1. Where possible, merge textually and incorporate the conceptual intent of the Chromium change, updating Fuchsia call sites if needed to support upstream improvements.
   - **Terse Summary Requirement:** After resolving (or deciding to skip) any commit that had merge conflicts, output a terse summary to the user explaining:
     1. What the commit does in the code, and
     2. How the conflict was resolved (what was kept/changed), or why the commit was skipped instead.
   - Once resolved, stage the changes and continue using the CLI:
     ```bash
     fuchsia-vendored-python src/media/third_party/chromium_media/.agents/skills/chromium-media-merge/scripts/apply_next_batch.py --continue <sha> --stop
     ```
4. **Skipping Irrelevant Commits:**
   - If an upstream commit is irrelevant to Fuchsia (e.g., macOS VideoToolbox, Windows DXVA, unmaintained codecs, or already incorporated changes), skip it:
     ```bash
     fuchsia-vendored-python src/media/third_party/chromium_media/.agents/skills/chromium-media-merge/scripts/apply_next_batch.py --skip <sha> --reason "<clear reason>" --stop
     ```
5. **Commit Message & Footer Preservation:**
   - Every imported Chromium commit is committed to `${FUCHSIA_REPO}` with its original subject and body, plus an explicit footer:
     ```
     [chromium_media] Upstream commit <sha>
     ```

---

## 3. Fixup Commits Policy

When build errors, compile warnings, lint failures, or test regressions are discovered after applying upstream Chromium commits (or during compile verification), **create separate, explicit fixup commits** (e.g., `[chromium_media] Fix build/test for upstream sync`) rather than amending or rewriting historical upstream merge commits.

This maintains a clear separation between 1:1 upstream patch import commits and Fuchsia-specific compatibility fixes.

---

## 4. Checkpointing & Crash Resilience

All state is tracked in the untracked `.merge_state/` directory inside `src/media/third_party/chromium_media/`:
- **`todo_commits.txt`**: Ordered list of remaining Chromium SHAs to apply.
- **`completed.log`**: Log of successfully merged or skipped SHAs and their reasons.
- **`maintained_files.txt`**: Active copy of the 53 maintained files list.

When any agent instance starts or resumes, it reads `.merge_state/todo_commits.txt` and continues from the exact SHA without duplication.

---

## 5. Verification Plan

### 1. Incremental Compile Check (Fast Driver and Codec Targets)
To quickly verify that `chromium_media` builds cleanly after incremental changes or conflict resolutions, build both the Amlogic decoder driver target and the VAAPI test package in a single `fx build` command so they can build in parallel in about the same wall-clock time:
```bash
fx build //src/media/drivers/amlogic_decoder:amlogic_decoder //src/media/codec/codecs/vaapi/test:vaapi_tests_package
```
*(Note: On non-x64 build directories like `sherlock`, building `vaapi_tests_package` requires adding it to your GN build arguments first via `fx add-test //src/media/codec/codecs/vaapi/test:vaapi_tests_package` or `--with-test` in `fx set`).*

### 2. Full Compile Check
Periodically after completing larger batches, run a full build across all Fuchsia targets:
```bash
fx build
```

### 3. Core Unit Tests
Run unit tests for the Amlogic decoder to verify core functionality:
```bash
fx test //src/media/drivers/amlogic_decoder/tests/unit_tests:amlogic-decoder-unittest
```

### 4. Hardware Integration Tests (Sherlock Target Device)
When testing on a Sherlock target device, run the full suite of H.264 decoder integration tests:
```bash
fx test amlogic-decoder-tests -- --logpath -
fx test use_h264_decoder_tests -- --logpath -
fx test use_h264_decoder_stream_switching_tests -- --logpath -
fx test use_h264_decoder_concurrent_stream_switching_tests -- --logpath -
fx test use_h264_decoder_mid_stream_change_tests -- --logpath -
fx test use_h264_decoder_mid_stream_change_fixed_buffers_tests -- --logpath -
fx test use_h264_decoder_mid_stream_change_fixed_buffers_no_realloc_tests -- --logpath -
fx test use_h264_decoder_frame_num_gaps_tests -- --logpath -
fx test use_h264_and_vp9_decoders_and_pcmm_stress_test -- --logpath -
```
*(Note: `JPEG` decoder suites are skipped as they are unmaintained. While `vaapi_tests_package` is built to verify compile-time compatibility of VAAPI code against `chromium_media`, running VAAPI tests on device is skipped on ARM64 hardware).*

---

## 6. Skill Scripts Reference

> [!IMPORTANT]
> **Working Directory Requirement:** All scripts should be run with your working directory (`cwd`) set to the agent-determined `${FUCHSIA_REPO}` directory so that `os.getcwd()` and relative paths resolve correctly.

All scripts are located in [`scripts/`](scripts/):
- **`apply_next_batch.py`**: The primary runner. Supports `--limit <N>`, `--continue <SHA>`, `--skip <SHA> --reason "<text>"`, and `--stop`.
- **`check_commit_diff.py`**: Helper to inspect the diff of an upstream Chromium commit across our maintained files. Supports `--stat-only <SHA>` or full diff `<SHA>`.
- **`merge_helpers.py`**: Shared utilities for git operations, path resolution, and state management.
- **`maintained_files.txt`**: Authoritative list of the maintained files in `chromium_media`.
