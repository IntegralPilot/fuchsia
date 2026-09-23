# Skill Linter

The `skill_linter` ensures `SKILL.md` files are well-formatted and contain
valid metadata. It is integrated with `fx format-code` and SHAC for automated
validation and fixing of skill documentation.

## Quick Start

### Run Locally

To check a directory for errors without modifying files:

```bash
PYTHONPATH=third_party/pyyaml/src/lib python3 \
    scripts/skill_linter/skill_linter.py /path/to/skill/dir
```

To automatically fix errors in-place:

```bash
PYTHONPATH=third_party/pyyaml/src/lib python3 \
    scripts/skill_linter/skill_linter.py --fixit /path/to/skill/dir
```

To output fixed content to stdout without modifying the file:

```bash
PYTHONPATH=third_party/pyyaml/src/lib python3 \
    scripts/skill_linter/skill_linter.py --suggest-fix /path/to/skill/dir
```

To output findings as structured JSON (for machine integration):

```bash
PYTHONPATH=third_party/pyyaml/src/lib python3 \
    scripts/skill_linter/skill_linter.py --suggest-fix-in-json /path/to/skill/dir
```

### Running Unit Tests

Unit tests for the linter are written using the standard Python `unittest`
framework and are integrated into the Fuchsia build system.

To run the tests using `fx test`:

```bash
fx test skill_linter_test
```

To run the tests directly with Python (useful for fast iteration):

```bash
python3 scripts/skill_linter/skill_linter_test.py
```

### Integration

- **`fx format-code`**: This is the primary way users interact with the
  linter. It runs `shac fmt`, which invokes the linter to automatically
  format and fix rule violations in the workspace.

- **CI/Presubmit**: SHAC runs the linter in check-only mode on the build
  bots. It identifies metadata and formatting errors, providing suggestions
  in the code review UI.

Note: The [skills.star](scripts/shac/skills.star) integration is configured to
scan for `SKILL.md` files within the `.agents/skills/` and several other
directories. This scope can be expanded to include other directories in the
future as needed.

## Validation Rules

### YAML Frontmatter

Metadata at the top of `SKILL.md` must be valid YAML and follow these
specific field constraints:

- **`name`**
    - **Constraint**: Must contain only lowercase letters, numbers, and
      hyphens.
    - **Length**: Maximum 64 characters.
    - **Safety**: Cannot contain XML or HTML tags.
    - **Auto-Fix**: The linter will convert casing, replace underscores with
      hyphens, strip invalid characters, and truncate the length to 64.

- **`description`**
    - **Constraint**: Maximum 1,024 characters and cannot be empty.
    - **Safety**: Cannot contain XML or HTML tags.
    - **Auto-Fix**: The linter strips XML tags and collapses multiple
      spaces. If the description exceeds 80 characters, it is dynamically
      converted to a YAML scalar block (`description: >`) for better
      readability.

### Markdown Body

The Markdown content following the frontmatter is automatically formatted to
ensure consistency across all skills:

- **Line Wrapping**: Body text is wrapped to fit within an **80-character**
  limit.

- **Smart Exclusions**: The formatter detects and preserves the layout of
  elements that should not be wrapped:

  - **Code Blocks**: Text inside triple backticks (`` ``` ``) is entirely
    excluded from formatting, including line wrapping and list item spacing
    adjustments, preserving it exactly as-is.
  - **Tables**: Entire table structures (headers, separator rows, and body
    cells) are detected and excluded from line length limits to avoid breaking
    table layout formatting.
  - **Headers**: Lines starting with `#` (ATX headers) are ignored.
  - **Blockquotes**: Lines starting with `>` are ignored.

- **Whitespace Hygiene**: Trailing whitespace is removed, and consecutive
  empty lines are collapsed into single breaks.

## Execution Modes

The linter supports several modes to accommodate different workflows:

- **Check-Only (Default)**
  - Reports errors and warnings to `stdout`/`stderr`.
  - Exits with `1` if any violations are found, and `0` otherwise.
  - Used for local manual checks and standard CI validation.

- **In-Place Fix (`--fixit`)**
  - Directly modifies the `SKILL.md` files in the filesystem.
  - Resolves metadata violations and reformats the Markdown body.

- **Suggested Fix (`--suggest-fix`)**
  - Applies fixes in memory and outputs the resulting full file content
    to `stdout`.
  - Useful for piping the output to a new file or diffing.

- **JSON Output (`--suggest-fix-in-json`)**
  - Outputs a structured JSON array of findings for machine consumption.
  - **Finding Structure**: Each finding object contains:
    - `filepath`: Relative path to the file.
    - `message`: A consolidated string of all findings (errors, warnings,
      and applied fixes).
    - `level`: Severity of the finding (`error` for fatal/blocking issues,
      `warning` for fixable violations).
    - `replacements`: (Optional) An array containing a single string
      representing the entire fixed file content.
  - **Error Handling**: Fatal parsing errors (e.g., malformed YAML or
    missing frontmatter) are reported as `error` level findings rather than
    script crashes.
  - **Exit Code Note**: In JSON mode, the script exits with `0`
    even if findings are reported. A non-zero exit code indicates a fatal
    script crash.

## Coding Design Patterns & Best Practices

When contributing to the `skill_linter` or its integrations, adhere to the
following patterns:

### 1. Interface Design: Machine-First

- **Centralized Severity**: The linter script is the source of truth for
  whether a violation is an `error` (blocking) or a `warning`
  (non-blocking). This logic is communicated via the JSON findings.
- **Structural Findings**: Use the `--suggest-fix-in-json` interface for all
  machine integrations (like SHAC). This avoids fragile exit code
  dependencies and allows passing rich metadata (like specific error
  messages per file).
- **Predictable Exit Codes**: Avoid using complex exit code schemes. Use `0`
  for success/processed and `1` for validation failure in human-readable
  modes.

### 2. Implementation Patterns: Parsing & Formatting

- **Safe YAML Loading**: Always use `yaml.safe_load()` for frontmatter
  parsing. Never use regex or manual string splitting to extract YAML
  values, as this is error-prone for complex keys or nested structures.
- **Paragraph-Based Formatting**: The Markdown formatter uses a "flush
  paragraph" pattern. Lines are buffered into paragraphs and wrapped
  collectively using `textwrap.TextWrapper`, while block-level elements
  (tables, code blocks) trigger immediate flushes to preserve their
  structure.
- **Pre and Post Processing**: Use `_pre_process` to standardize list
  indentation and `_post_process` for final whitespace hygiene. This keeps
  the core wrapping logic focused on text flow.

### 3. Python Documentation: Method Comments

- **Document All Methods**: All methods in `skill_linter.py` must contain a
  docstring or comment explaining their purpose, arguments, and return
  values. This ensures the tool remains maintainable and that its logic is
  transparent to both human developers and AI agents.

### 4. Integration & Environment

- **Dynamic Dependency Management**: The linter requires `pyyaml`, which is
  not in the standard Python library. The SHAC integration (`skills.star`)
  dynamically constructs the `PYTHONPATH` to include
  `third_party/pyyaml/src/lib`.
- **Logging Hierarchy**: Use the standard `logging` module instead of
  `print()`. This allows the tool to direct diagnostics to `stderr` while
  keeping `stdout` clean for formatted content or JSON findings.
- **Hermeticity**: Ensure the script remains hermetic and does not rely on
  global environment variables or local user configurations.

### 5. Testing & Verification

- **Unit Testing**: Use `skill_linter_test.py` for all core logic. Any
  change to validation regex or formatting rules must be accompanied by a
  corresponding test case.
- **Manual Verification**: Test new flags by creating a scratch directory
  with a `SKILL.md` and running the linter with the various modes described
  in the [Execution Modes](#execution-modes) section.
