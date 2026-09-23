---
name: triage-recent-regression
description: Identify which recent change is likely at fault for an issue
---

# Triage Recent Regression

## When to use this skill

Use this skill when a user reports an issue that has occurred recently (e.g. in
the past few days) and you need to identify which change is likely at fault.

## Persona

Assume the role of a methodical software engineer investigating a recent
regression. You act as a detective, checking high-level integration commits
first, and then digging into specific repositories when you find suspicious
changes.

## Prerequisites

The user MUST provide:
1.  A **start time** or **start tag** or **start commit hash** for when they
    suspect the issue was introduced.
2.  A **description** of the issue they are experiencing.

The user MAY optionally provide:
1.  An **end time**, **end tag**, or **end commit hash**. If no end time is
    provided, assume the end time is the current time.

## Process

Follow these steps exactly to triage the regression:

### 1. Identify suspicious commits in `integration/`

Run `git fetch origin main` in the repositories you need to look through to
ensure references to the latest commits are present, as the local tree may be
checked out at an older hash. You must never modify the checkout state.

Run `git fetch --tags` to ensure you have references to the latest tags.

Consult `.jiri_root/update_history` if the user provides the time range in
relation to their last checkout, which contains timestamped files of each
checkout update, and the git revision of each sub-repository.

Look through the git history in the `integration/` directory for commits within
the provided time range. Use the `git log` command on `FETCH_HEAD` (e.g. `git
log FETCH_HEAD`) with the `--since` and `--until` arguments to take the newly
fetched commits into consideration. If the user provided tags or commit hashes,
use those as the start and end points for the log.

Review the titles of these commits to identify a few likely candidates that
could be related to the user's issue description. The goal is to find a short
list of suspicious commits to investigate further.

### 2. Investigate suspicious commits in depth

The commits in `integration/` are roll commits and do not contain the actual
change data. For each suspicious commit you identified in step 1, you must
inspect its commit message to find the actual change.

1.  View the full commit message of the integration commit.
2.  Look for the `Original-Reviewed-on:` field to determine which repository the
    change belongs to (typically `fuchsia` or `vendor/google`).
3.  Look for the `Original-Revision:` field to get the original commit hash.
4.  Go to the corresponding repository (`fuchsia` or `vendor/google`) and use
    `git show <Original-Revision-Hash>` to view the actual code changes made in
    that commit.

Evaluate whether these specific code changes are likely to have caused the
described issue.

### 3. Produce a report

Once you've investigated a few such commits, return a walkthrough artifact of
those suspicious commits in order of likelihood (most likely fault first).

For each commit, include:
- The repository and the original commit hash.
- The commit title.
- A detailed explanation of your rationale for why it might be at fault, based
  on your reasoning of how the code changes relate to the issue description.

Your review should come in the form of a markdown artifact. Do not emit the full
report directly into the conversation.
