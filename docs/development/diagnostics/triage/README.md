# Triage

Note: Before attempting to use Triage, see go/fuchsia-subsystem-health.

Triage is a system for analyzing Fuchsia snapshots to detect predefined error
conditions and other states of interest. It uses configurable rules to scan
diagnostic data, such as Inspect data and logs, and to condense a large amount
of information into actionable warnings and reports.

Triage can be used with `ffx triage` and `fx triage` and is also used by
[Detect] to monitor devices in the field.

## Getting started

*   [Codelab: Using Triage]: A step-by-step tutorial on how to use
    `fx triage`, add new rules, and test them.

## Key concepts

*   [Configuring Triage]: A detailed reference for the `.triage` file
    format, including selectors, expressions, actions, and tests.
*   [`fx triage` Command Reference]: A reference for the
    `fx triage` command and its options.

[Detect]: /docs/development/diagnostics/analytics/detect.md
[Codelab: Using Triage]: codelab.md
[Configuring Triage]: config.md
[`fx triage` Command Reference]: fx_triage.md
