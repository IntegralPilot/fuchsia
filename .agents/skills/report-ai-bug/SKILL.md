---
name: report-ai-bug
description: >
  Files Buganizer issues to report bugs, failures, or unexpected behavior
  experienced while interacting with AI agents and skills in Fuchsia
  workflows. Use when the user wants to report an AI bug, file an issue about
  agent behavior, report something not working with an assistant or skill, or
  log a problem against an AI workflow. Don't use for reporting general
  Fuchsia OS/kernel bugs, hardware issues, or standard code review problems
  unless they are specifically about AI agent assistance.
---

# Report AI Bug

This skill guides the agent through a structured workflow to gather problem
details, extract technical context from the conversation history, present a
reviewable draft artifact to the user, and file a Buganizer issue against the AI
workflow team.

## Workflow Steps

When invoked, execute the following steps sequentially. Do not skip the user
review step.

### Step 1: Gather Problem Details from User

Collect the core details of the problem from the user:
1.  **Problem Experienced**: Ask the user what problem they experienced and what
    was not working as expected.
2.  **Expected Behavior**: Ask the user what they expected to have happen
    instead.

> [!TIP]
> **Avoid redundant questions**: If the user already described the problem and expected behavior in their initial request, do not re-ask. Extract what was provided and only ask for any missing pieces.

### Step 2: Investigate Conversation Context

Look back through the **current** conversation history and transcripts to
identify relevant context that applies to the user's reported problem.
- **Scope strictly to current session & subagents**: Only inspect the current
  conversation and any subagent conversations spawned by this current session
  (e.g., via `invoke_subagent`). Do **not** search across all past or unrelated
  agent conversations in the filesystem.
- Review recent turn transcripts, commands run, tool calls, error messages, or
  file modifications within this scope.
- Identify specific gotchas, tool failures, misunderstanding of prompts, or
  incorrect assumptions made by the agent.
- Summarize this technical context to substantiate the user's bug report. Why:
  Providing concrete logs and conversation traces enables the engineering team
  to reproduce and root-cause the agent failure without back-and-forth pinging.

### Step 3: Present Draft Artifact for User Review

Before filing any issue in Buganizer, generate a markdown artifact (e.g.,
`ai_bug_report_draft.md`) containing the complete draft of the bug report so the
user can review and comment on it.

When calling `write_to_file` to create the artifact, you MUST set
`ArtifactMetadata.RequestFeedback: true` and `UserFacing: true`. This renders a
review interface with a **Proceed** button for the user.

Format the artifact contents as follows:

```markdown
# AI Bug Report Draft

- **Title**: [Concise summary of the problem]
- **Component ID**: `1347088` (Note: Default target component for AI bugs; subject to change)
- **Priority**: P2 (Default)

## Problem Experienced / Actual Behavior
[Detailed description of what went wrong based on user input]

## Expected Behavior
[What the user expected to happen]

## Conversation & Technical Context
- **Command / Workflow Attempted**: [e.g., fx test, code review, skill invocation]
- **Relevant Logs / Error Output**:
  ```
  [Paste relevant error messages or tool outputs from conversation history]
  ```
- **Agent Trajectory Summary**: [Brief summary of where the agent went off-track in the conversation]
```

Ask the user to review the draft artifact, provide any edits or comments, and
approve it (or click **Proceed**) when ready to file.

### Step 4: File the Bug via Issues CLI Skill

Once the user approves the draft (or clicks Proceed), use the `issues-cli` (or
`buganizer-cli`) skill to file the issue in Buganizer.

- **Target Component**: `1347088`
- **Priority**: `P2`
- Follow the safe input guidelines from the `issues-cli` skill (using
  `--description_file` with a temporary file) to prevent shell escaping and
  markdown formatting breakage.

> [!CAUTION]
> **Explicit Authorization Required**: Creating an issue in Buganizer is a write operation that modifies external state. Do not run the create command until the user has explicitly approved the draft artifact.

#### Fallback (Skill Not Found)
If neither the `issues-cli` nor `buganizer-cli` skill is available in your
environment, do not attempt to guess CLI syntax or run raw commands. Instead,
inform the user that the bug filing skill is not available and ask them to file
the bug manually using the information provided in the approved draft artifact.

### Step 5: Confirm and Link Issue

After the issue is successfully created via the CLI:
1.  Extract the newly created Buganizer issue ID from the command output.
2.  Provide the user with the issue ID and confirmation that the bug has been
    filed.
