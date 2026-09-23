# Initializing `.merge_state/` for Chromium Media Merge

This procedure describes how to create and initialize the untracked `.merge_state/` directory and generate the ordered commit queue (`todo_commits.txt`) when starting a fresh sync session.

> [!IMPORTANT]
> **When to Use This Guide:** Only perform this procedure when `.merge_state/` does not already exist or when starting a brand new sync from scratch. If `.merge_state/todo_commits.txt` already exists and contains pending commits, **do not re-initialize**; proceed directly to the merge loop in `SKILL.md`.

---

## 1. Identify the Base Revision

Read `${FUCHSIA_REPO}/src/media/third_party/chromium_media/README.fuchsia` to determine the upstream Chromium commit hash that `chromium_media` was last synced to.

Look for the `Revision:` line:
```
Revision: c53c9bb7825c3179a7e89607e8520438fb2bf627
```
Note this commit hash from the `Revision:` line (for example, `c53c9bb7825c3179a7e89607e8520438fb2bf627`). You will substitute this hash for `<base_sha>` when running the commit enumeration command in Section 3. The target revision (`<target_sha>`) is usually upstream Chromium's `HEAD` (ToT), or a specific target commit.

---

## 2. Create the `.merge_state/` Directory

Create the `.merge_state/` untracked state directory inside `${FUCHSIA_REPO}/src/media/third_party/chromium_media/`:

```bash
mkdir -p "${FUCHSIA_REPO}/src/media/third_party/chromium_media/.merge_state"
```

---

## 3. Enumerate Upstream Chromium Commits (`todo_commits.txt`)

Run `git log` in the upstream Chromium repository (`CHROMIUM_REPO_MEDIA` or `${FUCHSIA_REPO}/local_src/chromium/src/media`) to find all commits between `<base_sha>` and `<target_sha>` (`HEAD`) that touch any of the maintained files defined in the skill (`scripts/maintained_files.txt`).

Use `--reverse` to order the commits chronologically from oldest to newest:

```bash
# Example assuming Chromium is at ${FUCHSIA_REPO}/local_src/chromium/src/media
# Replace <base_sha> with the hash from README.fuchsia (e.g., c53c9bb7825)

git -C "${FUCHSIA_REPO}/local_src/chromium/src/media" \
    log --oneline --reverse c53c9bb7825..HEAD -- \
    $(cat "${FUCHSIA_REPO}/src/media/third_party/chromium_media/.agents/skills/chromium-media-merge/scripts/maintained_files.txt") \
    | awk '{print $1}' > "${FUCHSIA_REPO}/src/media/third_party/chromium_media/.merge_state/todo_commits.txt"
```

---

## 4. Initialize `completed.log`

Create an empty `completed.log` file to record successfully merged or skipped commits:

```bash
touch "${FUCHSIA_REPO}/src/media/third_party/chromium_media/.merge_state/completed.log"
```

---

## 5. Verify Initialization

Verify that `.merge_state/todo_commits.txt` contains a non-empty list of commit hashes:
```bash
head -n 5 "${FUCHSIA_REPO}/src/media/third_party/chromium_media/.merge_state/todo_commits.txt"
```

Once `.merge_state/` is initialized, return to `SKILL.md` and begin the incremental patch application workflow using `apply_next_batch.py`.
