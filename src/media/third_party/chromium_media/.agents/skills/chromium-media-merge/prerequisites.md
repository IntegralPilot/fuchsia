# Chromium Media Merge Prerequisites

Before starting or resuming any `chromium-media-merge` workflow, the agent **MUST** execute the checks described below. Do not run any merge scripts or apply any patches until these checks pass.

---

## 1. Determine & Validate `FUCHSIA_REPO`

The workflow requires a resolved absolute path to the root of the Fuchsia repository (`FUCHSIA_REPO`).

### Discovery & Fallback Strategy
1. Check if the `FUCHSIA_REPO` environment variable is defined.
2. If `FUCHSIA_REPO` is **not** defined, discover the repository root using the editor's current workspace directory or currently-open file:
   ```bash
   # Run git rev-parse from the directory of the currently-open file or active workspace
   git rev-parse --show-toplevel
   ```
   Set `FUCHSIA_REPO` to the resulting path for all subsequent commands in this workflow.

### Mandatory Working Directory (`cwd`) Rule
When executing any skill script (such as `apply_next_batch.py`) or git command, you **MUST run the command with your working directory (`cwd`) set to the agent-determined `${FUCHSIA_REPO}` directory**. This ensures that script path fallbacks (which check `os.getcwd()`), relative Fuchsia paths (`src/media/third_party/chromium_media/...`), and git operations resolve consistently against the correct Fuchsia repository.

### Mandatory Validation Check
If `FUCHSIA_REPO` is already defined in the environment, check whether it corresponds to the git repository root of the user's currently-open file or active workspace.
- **If `FUCHSIA_REPO` does NOT match the repository of the currently-open file/workspace:**
  - **STOP IMMEDIATELY.** Do not execute any merge scripts or commands.
  - Inform the user of the mismatch between `$FUCHSIA_REPO` and their active workspace/file.
  - Ask the user which Fuchsia repository path should be used before proceeding.

---

## 2. Verify Chromium Source Repository Location (`chromium/src/media`)

The workflow scripts require access to an upstream Chromium git repository to export patches, examine commit histories, and perform 3-way file merges (`git show <sha>:path`, `git diff-tree`, etc.).

### Expected Default Path
- `${FUCHSIA_REPO}/local_src/chromium/src/media`
- Alternatively, if the `CHROMIUM_REPO_MEDIA` environment variable is explicitly set, use that path.

### Mandatory Agent Check Step
Run the following command to verify that the upstream Chromium `media` repository exists and is a valid git repository:

```bash
git -C "${FUCHSIA_REPO}/local_src/chromium/src/media" rev-parse --is-inside-work-tree
```

### Remedy Instructions (If Check Fails)
If the command fails (directory does not exist, not a git repository, or permission error):
1. **STOP IMMEDIATELY.** Do not attempt to run `apply_next_batch.py` or any git apply commands.
2. **Inform the User** that the upstream Chromium repository was not found at `${FUCHSIA_REPO}/local_src/chromium/src/media`.
3. **Ask the User to Remedy This:**
   - Instruct them to clone, fetch, or symlink their Chromium checkout so that `local_src/chromium/src/media` exists under `${FUCHSIA_REPO}`.
   - Or instruct them to export `CHROMIUM_REPO_MEDIA=/path/to/chromium/src/media`.
4. **Wait for User Confirmation** before re-running the check and continuing.

---

## 3. Verify Fuchsia Workspace State

Ensure that the Fuchsia workspace at `${FUCHSIA_REPO}` is ready for incremental patch application:
1. Check that `src/media/third_party/chromium_media` exists in `${FUCHSIA_REPO}`.
2. Check `git status -s` in `${FUCHSIA_REPO}/src/media/third_party/chromium_media`.
   - If uncommitted changes exist outside of an intentional `.merge_state/` directory or an ongoing conflict resolution step, ask the user whether to stash, commit, or revert them before proceeding.
3. Verify that `fx` and standard development tools (`fuchsia-vendored-python`, `git`) are available in the PATH.
