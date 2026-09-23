<!-- Copyright 2026 The Fuchsia Authors. All rights reserved. -->
<!-- Use of this source code is governed by a BSD-style license that can be -->
<!-- found in the LICENSE file. -->

# FFX Plugin Error Construction Guide

This document provides a comprehensive manual for `ffx` plugin authors on how to construct, format, and propagate errors within the `ffx` multi-tool architecture framework.

## Core Philosophy: User-Facing Errors vs. Internal Bugs

The `ffx` front-end main execution loop processes all errors returned from plugins using `anyhow::Error` objects. The runtime framework evaluates the error object payload using downcasting:

1.  **Actionable User Errors (`FfxError`)**: If the error downcasts successfully to `FfxError::Error`, the framework treats it as an expected operational failure. The message is printed directly to the user terminal `stderr` cleanly, free of engineer stack traces.
2.  **Unexpected Bugs**: All other unmapped error types (e.g., raw network errors, filesystem errors, un-downcast `anyhow::Error`) are treated as internal tool **BUGS**. The framework automatically attaches a `BUG:` stack trace prefix and instructs the user to file a Buganizer ticket at `go/ffx-bug`.

**Rule of Thumb**: If the failure is caused by bad user input, missing local file configurations, target unavailability, or anything actionable by the end-user, it **MUST** be returned as an explicit `FfxError` using the macro utilities below.

## Error Macro Utilities

The errors library crate (`ffx_error`) provides four main macro entries optimized for plugin development:

### 1. `ffx_error!`
Use this to construct a standalone `FfxError` instance with a simple text string. By default, it associates the failure with an exit status code of `1`.

```rust
use errors::ffx_error;

// Plain string message error
let err = ffx_error!("Target device socket connection refused.");

// Formatted template message error
let err_fmt = ffx_error!("Failed to open file: {}", path.display());
```

### 2. `ffx_error_with_code!`
Use this when the subcommand needs to return a specific non-zero exit status code back to the host shell wrapper script layer.

```rust
use errors::ffx_error_with_code;

// Returns a custom exit code 2 indicating entry mapping target missing
let err = ffx_error_with_code!(2, "Configuration target key not found.");
```

### 3. `ffx_bail!`
A highly convenient control flow macro that constructs an `ffx_error!`, wraps it in an `Err(...)` enum variant, and instantly triggers an early return (`return Err(...)`) from the active function context block.

```rust
use errors::ffx_bail;

if !manifest_path.exists() {
    ffx_bail!("Staged flashing manifest path '{}' does not exist.", manifest_path.display());
}
```

### 4. `ffx_bail_with_code!`
Combines custom exit status status codes with immediate control flow bailing early termination.

```rust
use errors::ffx_bail_with_code;

if value.is_null() {
    ffx_bail_with_code!(2, "Configuration target key contains no value data mapping.");
}
```

## Structuring Descriptive Error Strings

To guarantee optimal user ergonomics, all error text blocks should follow the imperative style guide rules:
*   **Context First**: Explicitly detail *what* operation failed before naming the low-level trigger.
*   **Actionable Hints**: Provide explicit instructions or alternative parameters commands if possible (e.g., `"Run ffx doctor to diagnose connection state."`).
*   **No Engineering Traces**: Keep raw logs, stack dumps, and module path identifiers inside the background file logging layers (`ffx.log`), preserving clean streams for the user console.
