---
name: fuchsia-gerrit-cli
description: >
  Guide for using the `fx gh` command-line interface to interact with Fuchsia
  Gerrit changes (CLs). Use when reading open changes, posting inline
  comments, replying to reviewer threads, managing draft comments, checking CI
  status with `pr checks`, or updating CQ labels from the terminal.
---

# Fuchsia Gerrit CLI (fx gh)

## Overview

The `fx gh` tool provides a command-line interface for managing Gerrit code
reviews that matches the GitHub CLI (`gh`). Coding agents can leverage their
parametric knowledge of the `gh` tool to use `fx gh`.

`fx gh` provides Fuchsia specific logic for interacting with Gerrit code
reviews, and it is highly preferred to raw REST API calls. However, if `fx gh`
is NOT available in your environment (e.g., for external contributors), you may
safely fall back to using the Gerrit REST API via `curl`.

## Key Concepts and Differences

While the interface is the same as `gh`, it interacts with Gerrit. Here are the
key mappings and differences you need to know:

### 1. Pull Requests are Gerrit Changes (CLs)
- In `fx gh`, a "Pull Request" (PR) corresponds to a Gerrit Change (commonly
  referred to as a CL in Fuchsia).
- Commands like `fx gh pr list`, `fx gh pr view`, and `fx gh pr comment` work on
  Gerrit changes.

### 2. Identifying Changes
- `fx gh` works with Gerrit change IDs.
- You can use the numeric ID from the URL (e.g., `1569017` from
  `https://fuchsia-review.git.corp.google.com/c/fuchsia/+/1569017`).
- You can also use the `Change-Id` value from the commit message (e.g.,
  `Id102da53bb4ebe08aa599f1479650553fb3f118d`).
- **Important**: The `Change-Id` line in the commit message is critical for
  Gerrit to link subsequent commits back to the same code review. Do not remove
  or alter it unless intentionally creating a new change.

### 3. Command Specifics

#### `pr list`
- **Assignee vs Reviewer**: Gerrit does not have a direct equivalent to GitHub's
  "assignee". The `-a, --assignee` flag is mapped to filter by **reviewer** in
  Gerrit.
- To filter by author/owner, use the `--author` flag.
- **Advanced Searching**: The `--search` flag allows access to all of Gerrit's
  advanced search functionality.
  - **Search Operators**: Common and useful Gerrit operators include:
    - `message:` / `subject:`: Match keywords or patterns (e.g.,
      `message:"fixing a bug"` or `message:^.*regex.*`) in the commit message or
      subject.
    - `file:` / `path:`: Match affected files by exact path or regex (e.g.,
      `file:src/main.rs` or `file:^.*\.rs`).
    - `owner:`, `reviewer:`, `cc:`: Match based on users involved in the CL.
    - `age:`: Filter by the time since the last update (e.g., `age:2d`,
      `-age:1w`).
    - `has:`: Filter by properties like `has:unresolved`, `has:draft`.
    - `is:`: Filter by state or properties like `is:open`, `is:wip`,
      `is:mergeable`.
    - `label:`: Match by label votes (e.g., `label:Code-Review+2`).
    - See https://gerrit-review.googlesource.com/Documentation/user-search.html
      for complete documentation.
  - **Direct Query Pass-Through**: The search string is appended directly to the
    Gerrit REST API query, fully supporting explicit boolean operations (`AND`,
    `OR`, `NOT`), grouping parentheses, and negation (e.g., `-is:wip`).
  - **Combination With Flags**: Any flags supplied alongside `--search` (like
    `--author` or `--label`) are joined with a space (implicitly acting as an
    `AND` in Gerrit search syntax).
  - **Important Caveat (`--state`)**: By default, `status:open` is prepended to
    the search query. If you use custom status operators (e.g., `status:merged`
    or `status:abandoned`) within `--search`, they will conflict and yield no
    results. **Workaround**: Pass `--state=all` along with `--search` to take
    full control over the status filter.

#### `pr comment`
- **Threading and Replying to Comments**: When posting a comment with `--path`
  and `--line`, the tool automatically detects existing threads on that line and
  replies to the latest comment. You do **not** need to manage thread IDs
  manually.
- If there are multiple independent threads on the same line, the tool replies
  to the one with the most recent activity.
- **Example**: To reply to a review comment on line 42 of `src/foo.cc`:
  ```bash
  fx gh pr comment 1569017 --path src/foo.cc --line 42 -m "Done. Fixed as suggested." --resolved
  ```
- **Draft Comments**: You can use the `--draft` flag to save a comment as a
  draft on Gerrit without immediately publishing it:
  ```bash
  fx gh pr comment 1569017 --path src/foo.cc --line 42 -m "WIP fix coming" --draft
  ```
  If you need to post multiple comments, you must use the `--draft` flag for
  each comment, and then publish them all at once.
- **CRITICAL**: When responding to comments on a Gerrit code review, you MUST
  reply within the specific comment thread using the `--path` and `--line` flags
  of `fx gh pr comment`. Do NOT leave a top-level unthreaded comment unless
  explicitly requested.

#### `pr edit` and `pr review`
- **Approving a Change**: To add a `Code-Review+2` vote, use `fx gh pr review
  <id> --approve`.
- **Triggering AI Review Agents**: To trigger an AI review agent or list
  available ones, use `fx gh pr review` with `--agent` or `--list-agents` flags.
  - **List Agents**: `fx gh pr review <id> --list-agents`
  - **Trigger Agent (Default)**: `fx gh pr review <id> --agent <agent_id>` (This
    posts drafts by default. You do not need to take any additional action to
    post them).
  - **Trigger Agent (Dry Run)**: `fx gh pr review <id> --agent <agent_id>
    --dry-run` (This prints suggestions to terminal without posting drafts).
- **Triggering CQ (Commit Queue)**: To set Gerrit labels like `Commit-Queue+1`
  or `Commit-Queue+2`, you must use the `edit` subcommand with the `--add-label`
  flag.
  - **Example**: `fx gh pr edit <id> --add-label Commit-Queue+1`

#### `pr checks`
- **Viewing Check Status**: To see the status of CI checks and failing bots for
  a Gerrit change, use `fx gh pr checks <id>`. This command provides the
  detailed logs and failure reasons needed to debug failing checks.

#### `pr cherry-pick`
- **Cherry-picking a CL**: To fetch and apply a Gerrit change to your current
  local Git branch, use `fx gh pr cherry-pick <id>`.

### 4. Uploading Changes
- You do **not** need to use `fx gh` to upload or update CLs.
- Use the normal `git push` commands to upload changes to Gerrit (e.g., `git
  push origin HEAD:refs/for/main`).

## Usage Examples

- View a change: `fx gh pr view 1569017`
- List open changes: `fx gh pr list --author me`
- Comment on a specific line: `fx gh pr comment 1569017 --path src/foo.cc --line
  42 -m "Looks good"`
- Mark a CL for CQ dry-run: `fx gh pr edit 1569017 --add-label Commit-Queue+1`
- Cherry-pick a change locally: `fx gh pr cherry-pick 1569017`
- View check status: `fx gh pr checks 1569017`
