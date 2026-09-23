---
name: fuchsia-readme-generator
description: >
  Guidelines and workflow for generating or updating README.md files for
  Fuchsia, with Fuchsia-specific developer and agent record-keeping.
---

# Fuchsia README Generator Skill

This skill guides you in generating a new `README.md` or updating an existing
one for a project within the Fuchsia codebase.

## When to use this skill

Use this skill ONLY when:
1.  A developer explicitly asks you to write or update a `README.md` file.
2.  You are refactoring a project and want to ensure its primary `README.md` is
    updated to reflect architectural or usage changes.

For detailed rules, formatting standards, and style guidelines when editing
`README.md` files, see the [`edit-fuchsia-doc`](../edit-fuchsia-doc/SKILL.md)
skill.

Do NOT use this skill for other types of markdown documentation (such as design
documents, tutorials, API specs, or user guides), which may have different
styling and structure rules.

## Process

### 1. Context Gathering

Gather context about the project before drafting the README:
* **Project Location & Security:** Identify the project path (e.g.,
  `//src/connectivity/...`).
  * **WARNING:** Do not include any information specific to files,
    repositories, or documentation under the `//vendor` directory. The
    `//vendor` directory may contain proprietary or private information that
    must remain confidential and should never be leaked into general or
    public-facing documentation.
* **Build Targets:** Analyze `BUILD.gn` or `BUILD.bazel` for target names, test
  targets, and FIDL dependencies.
* **Diagnostics & Observability:** Analyze source code to identify main entry
  points (e.g., `main.rs`, `main.cc`) and determine if the project exposes
  system diagnostics via the Inspect API or writes structured logs (look for
  inspect or logging libraries in the source). Documenting these telemetry
  interfaces is critical for inspecting system health and troubleshooting
  failures with `ffx inspect` and `ffx log`.

### 2. Fuchsia-Specific README Structure & Record-Keeping

For projects in the Fuchsia codebase, the `README.md` serves as a critical system
of record for developers and AI agents. Structure the README with relevant
Fuchsia-specific sections:

1.  **Fuchsia Build Configuration:**
    * Provide the exact `fx set` command to include the project and its tests.
    * *Example:* `fx set workstation_eng.x64 --with
      //src/my/project,//src/my/project:tests`
2.  **Topology & Lifecycle (if applicable):**
    * Explain where the component sits in the component topology (realms,
      monikers) and how it is started or routed.
3.  **FIDL Boundaries (if applicable):**
    * If the project exposes or interacts with FIDL APIs, explicitly list and
      link the FIDL protocols implemented or consumed. This is vital for agents
      and developers tracking API dependencies.
4.  **Diagnostics & Debugging (if applicable):**
    * Document the exact commands to watch logs: `ffx log --set-severity
      core/my-project#DEBUG`
    * Document the exact commands to inspect state: `ffx inspect show
      core/my-project`
5.  **AI Agent Record-Keeping (Developer Notes):**
    * Document quirks, known issues, or architectural decisions (e.g., why a
      specific synchronization pattern or constraint was chosen). This
      prevents future AI agents and contributors from re-introducing bugs or
      using incorrect APIs.
    * *Example:* "Note: Do not use async tasks here due to X constraint."
6.  **Tracked Design Patterns & Best Practices:**
    * Document specific design patterns, best practices, or style conventions
      highlighted by developers in code review feedback or comments to preserve
      institutional knowledge.

### 3. Fuchsia Formatting & Conventions

When editing or formatting `README.md` files, follow the rules and style
guidelines in the [`edit-fuchsia-doc`](../edit-fuchsia-doc/SKILL.md) skill.
Key conventions include:

* **80-Character Line Limit:** Manually wrap all body/prose text to a strict
  **80-character limit per line**. Most formatters (such as `fx format-code`)
  do NOT wrap prose lines automatically in the Fuchsia codebase.
  * *Exclusions:* Do not wrap code blocks (triple backticks), markdown tables,
    headers (`#`), or blockquotes (`>`).
* **Tooling:** Use modern `ffx` commands rather than legacy tools.
* **Links:** Use relative links for in-tree files to maintain documentation
  integrity.
* **Minimal-Disruption Restructuring:** When updating an existing `README.md`,
  preserve the original text content as much as possible, moving existing
  blocks into the appropriate sections rather than rewriting them
  unnecessarily.

### 4. Verification

* **Do Not Block on Verification:** Deliver the initial draft of the `README.md`
  immediately using static analysis. Never block draft delivery on running a
  build or a test.
* **State Verification Status:** Clearly inform the developer whether the
  documented commands (e.g., `fx test`, `fx set`) have been verified in the
  workspace or derived statically from build definitions.
