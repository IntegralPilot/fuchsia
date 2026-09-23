---
name: edit-fuchsia-doc
description: >
  Edits and copyedits Fuchsia documentation (.md) files to comply with the
  official Fuchsia documentation style guide and standards. Use when editing,
  formatting, reviewing, or refactoring Fuchsia Markdown documentation.
---

# Edit Fuchsia Doc

Edits and copyedits Fuchsia documentation files (`.md`), ensuring full
compliance with the official Fuchsia documentation style guide, formatting
standards, and tone guidelines.

The official source of truth for the Fuchsia documentation style guide is
located at
[`/docs/contribute/docs/documentation-style-guide.md`](/docs/contribute/docs/documentation-style-guide.md).

## Core Style Guide Principles & Guidelines

When editing any Fuchsia documentation file, enforce the following standards:

### 1. Top-Level YAML Frontmatter

- **Do NOT edit top-level YAML frontmatter**: Never modify, reformat, or wrap
  the top-level YAML frontmatter block at the top of a Markdown document (e.g.,
  `keywords: ...` or `description: ...`). Frontmatter metadata generation and
  formatting should be handled by a different skill. Only edit the Markdown body
  below the frontmatter block.

### 2. 80-Character Line Limit

- Wrap all prose lines at a maximum length of **80 characters**.
- **Exception**: Long URLs and reference link definitions should remain on a
  single line without wrapping.

### 3. Reference-Style Links & Link Formatting

- Prefer **reference-style links** (`[link text][reference-id]`) in body text
  with definitions (`[reference-id]: url`) placed at the bottom.
- Do NOT include `{:.external}` in reference link definitions (e.g.
  `[reference-id]: url`), as `{:.external}` breaks reference links.

    Bad example:

    ```
    [fuchsia-overview]: /docs/concepts/README.md {:.external}
    [style-guide]: /docs/contribute/docs/documentation-style-guide.md {:.external}
    [get-started]: /docs/get-started/README.md {:.external}
    ```

    Good example:

    ```
    [fuchsia-overview]: /docs/concepts/README.md
    [style-guide]: /docs/contribute/docs/documentation-style-guide.md
    [get-started]: /docs/get-started/README.md
    ```

- **Do NOT put example reference link definitions in fenced Markdown code
  blocks**:
    - When writing or editing documentation that includes **examples of
      reference link definitions** (e.g. illustrating `[id]: url` in a syntax
      guide or code snippet), **do NOT wrap them in standard fenced Markdown
      code blocks (` ```none `)**.
    - DevSite's Markdown engine (Hoedown) strips any line matching `^[id]: url`
      across the entire document during its pre-processing pass-even inside
      fenced code blocks. This breaks code block delimiter tracking (`in_code ==
      True`), causes unclosed HTML/code blocks, and overwrites real reference
      link definitions.
    - **Correct Syntax**: Use HTML `<pre><code>` blocks and escape the opening
      square bracket with the HTML entity **`&#91;`** (e.g.
      `<pre><code>&#91;example-id]: https://example.com</code></pre>`). Because
      `&#91;` does not start with a literal `[`, Hoedown never matches or strips
      the line during pre-processing, and the browser renders it cleanly as
      `[example-id]: https://example.com`.

- Use relative paths or full paths in the codebase for source tree docs (e.g.,
  `/docs/concepts/README.md`) and canonical URLs for reference docs.
- **Do NOT use `file://` URLs**: Never use `file://` for linking a file. Always
  use the full path in the codebase (e.g., `/docs/concepts/README.md`).
- **Do NOT use `mailto:` URLs**: Do not create `mailto:` links or turn email
  addresses / usernames into links (e.g., do not write
  `[user@example.com][user-email]` with `[user-email]:
  mailto:user@example.com`). Mention email addresses and usernames as plain text
  or inline monospace code (e.g., `user@example.com` or `username`).

### 4. Sentence Case Headers & Custom Anchors

- All titles and section headers (`#`, `##`, `###`) MUST use **sentence case**
  (e.g. `## Document locations`, not `## Document Locations`).
- Use `{:#anchor-name}` for custom section anchors (e.g. `## Section title
  {:#section-title}`).
- Custom section header (`##`, `###`) anchors MUST use **dashes (`-`)** instead
  of underscores (`_`).
- The main page title (level 1 heading `#`) does NOT need a custom anchor
  (remove any custom anchor like `{:#anchor-name}` from `# Title`).

### 5. Code Block Syntax

- Use `posix-terminal` as the code block language identifier for fenced Markdown
  code blocks containing runnable shell command examples. Do NOT hardcode a
  leading `$` prompt character in commands, as `posix-terminal` renders it
  automatically.
- Use `none` (or `none {:.devsite-disable-click-to-copy}`) for output logs or
  code samples that should not be copied.
- **Do NOT put example reference link definitions (`[id]: url`) in fenced
  Markdown code blocks**: Always format example reference link definitions using
  HTML `<pre><code>` blocks with the opening bracket escaped as **`&#91;`**
  (e.g., `<pre><code>&#91;example-id]: https://example.com</code></pre>`).
  Fenced code blocks (` ``` `) do not prevent Hoedown from stripping reference
  link definitions during pre-processing, which breaks page rendering.

### 6. Callout Boxes (Alerts & Notes)

- **Use DevSite Markdown callout syntax**: To create callout boxes (such as
  notes, warnings, tips, or cautions) on fuchsia.dev, start a paragraph with one
  of the supported DevSite callout keywords followed by a colon (`:`):
    - `Note: ...`
    - `Caution: ...`
    - `Warning: ...`
    - `Important: ...`
    - `Tip: ...`
    - `Key Point: ...`
    - `Key Term: ...`
- **Do NOT use bad callout box syntax**:
    - Do NOT use GitHub-style blockquote alerts (e.g., `> [!NOTE]`, `> [!TIP]`,
      or `> [!WARNING]`). DevSite does not support them.
    - Do NOT use bolded or italicized lead-ins (e.g., `**Note:**` or `_Note:_`).
      These render as normal body text rather than a callout box.
    - Do NOT use raw HTML `<aside>` tags in standard Markdown pages unless
      required by a specific documentation widget.

### 7. Horizontal Rules & Separators

- Do NOT use `---` horizontal rules or separators in Markdown documents.

### 8. Lists & List Item Spacing

- In bulleted lists and numbered lists, include an empty line between items for
  readability and proper rendering on fuchsia.dev.
- Include an empty line between a parent list item and the start of a sub-list.

### 9. Tone, Voice, and Grammar

- **Plain U.S. English**: Write clear, direct English using simple words and
  standard contractions (_it's_, _don't_).
- **Second Person ("you")**: Write to the reader as "you". Avoid third-person
  ("users can") or first-person plural ("we can").
- **Present Tense & Active Voice**: State facts in the present tense (avoid
  future tense "will"). Use active voice ("The command creates a file") over
  passive voice ("A file is created by the command").
- **Gender-Neutral Pronouns**: Use singular *they/them/their*.
- **Acronyms & Terms**: Spell out acronyms on first mention (e.g. "Looks Good To
  Me (LGTM)"). Ensure terminology matches the Fuchsia glossary.

### 10. Top-Level Outline & Sub-section Navigation Lists

- Include a **top-level outline navigation list** near the top of the guide
  (after the introduction and before the first `##` section).
- Include a **sub-section step list** at the top of major sections (`##`
  headings) that contain multiple sub-sections (`###`).
- **Keep top-level outlines non-redundant**:
    - The top-level outline navigation list should list only major sections
      (`##` headings).
    - Do NOT nest sub-section (`###`) links under major section items in the
      top-level outline if those sub-sections are already listed at the top of
      their respective section. Keep the top outline clean and non-redundant.
- Use **numbered list items** (`1. [Section title](#anchor)`) ONLY when sections
  are meant to be executed in sequence, such as a tutorial or how-to guide. For
  sequential procedures, precede the list with "The steps are:".
- Use **bulleted list items** (`* [Section title](#anchor)`) for non-sequential
  topics, such as an index page, overview, or reference document. Do NOT include
  "The steps are:" when the sections are non-sequential.

    Top-level outline example (sequential procedure):

    ```
    The steps are:

    1. [Prerequisites](#prerequisites)
    2. [Build Fuchsia](#build-fuchsia)
    3. [Set up Device Cloud](#set-up-device-cloud)
    4. [Troubleshooting](#troubleshooting)
    ```

    Top-level outline example (non-sequential reference or index):

    ```
    * [Overview](#overview)
    * [Available skills](#available-skills)
    ```

    Sub-section step list example:

    ```
    ## Set up Device Cloud {:#set-up-device-cloud .numbered}

    1. [Set up environment](#set-up-environment)
    2. [Check out device](#check-out-device)
    3. [Recover and flash](#recover-and-flash)
    4. [Serve packages](#serve-packages)
    ```

    Troubleshooting sub-section list example:

    ```
    ## Troubleshooting {:#troubleshooting}

    - [Reset to bootloader](#reset-to-bootloader)
    - [Disable RAM dump](#disable-ram-dump)
    - [Full recovery](#full-recovery)
    - [Hash verification failure](#hash-verification-failure)
    ```

## Editing Workflow

1.  **Back Up Target Document**: Before making any edits, copy the original
    target documentation file to the local `/tmp` directory on the host machine
    (for example, `/tmp/<filename>.bak`). This backup allows easy reversal if
    the user does not like the edits performed by the skill.

2.  **Inspect Target Document**: Scan target Markdown file for violations:
    - Lines exceeding 80 characters (excluding top-level YAML frontmatter).
    - Header titles using Title Case instead of sentence case.
    - Main title (`# ...`) containing custom anchor, or section headers using
      underscore-based anchors.

    - Email addresses or usernames formatted as `mailto:` links.

    - File links using `file://` scheme instead of full paths in the codebase.

    - Top-level outline list containing redundant nested sub-section items.

    - Missing top-level outline navigation list or sub-section step lists.

    - Bad callout box syntax (e.g., `> [!NOTE]`, `**Note:**`, `_Note:_`).

    - Presence of `---` horizontal rules / separators.

    - Bulleted or numbered list items without empty lines between them.

    - Inline links instead of reference-style links, or reference link
      definitions containing `{:.external}`.

    - Shell code blocks using `sh` or hardcoded `$` prompts instead of
      `posix-terminal`.

    - Passive voice, future tense ("will"), or third-person phrasing.

3.  **Apply Copyediting Changes**:
    - Do NOT edit, reformat, or wrap text in top-level YAML frontmatter.
    - Refactor prose into active, second-person ("you"), present-tense
      sentences.
    - Update headers to sentence case, remove custom anchor from `#` main title,
      and use dash-based anchors for subheadings (e.g., `{:#anchor-name}`).
    - Add a top-level outline navigation list after the intro (listing major
      sections without redundant sub-section nesting), and sub-section lists
      under major section headings.
    - Replace any `file://` link URLs with the full path in the codebase.
    - Replace any `mailto:` links for email addresses/usernames with plain text
      or code.
    - Convert bad callout syntax (`> [!NOTE]`, `**Note:**`) to DevSite callout
      syntax (e.g., `Note: ...`, `Tip: ...`).
    - Remove all `---` horizontal rules / separators.
    - Add empty lines between list items in bulleted and numbered lists.
    - Wrap prose lines at 80 characters while preserving single-line URLs.
    - Convert inline links to reference links at the end of the file (do NOT add
      `{:.external}` to reference link definitions).

    - Update shell code blocks to `posix-terminal` syntax.

4.  **Validate & Review**:
    - Verify line lengths (all prose <= 80 chars).
    - Ensure all link reference tags match their definitions at the bottom.

5.  **Stage & Preview Document**:
    - Run `fx doc-preview <path-to-target-md-file>` to stage the edited
      documentation file to DevSite.
    - Note: `fx doc-preview` is an internal tool for staging documentation on
      DevSite, so not all developers may have access to it.
    - After staging completes, present the staging preview links (the short link
      `go/fuchsia-doc-preview/<user>` and the
      `https://fuchsia.devsite.corp.google.com/preview/<user>` URLs) to the user
      so they can review the rendered page.

6.  **Report Backup & Recovery Instructions**: After finishing the skill, inform
    the user about the existence of the backup copy in the `/tmp` directory and
    provide instructions on how to recover the original target document if
    desired (for example, `cp /tmp/<backup_file> <target_doc_path>`).
