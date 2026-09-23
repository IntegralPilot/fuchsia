---
name: migrate-logging
description: >
  Migrate a DFv2 C++ driver's logging from the legacy FDF_LOG/DF_LOG/FDF_LOGL
  macros to the modern fdf:: logger (fdf::info/error/warn/debug/trace). Use
  when a driver still calls FDF_LOG or DF_LOG with printf-style format strings
  (%s, %d, %08x) that must become std::format-style {} placeholders, including
  the //sdk/lib/driver/logging/cpp dep and zx::result formatter rules. DFv2
  drivers only -- confirm the driver is not DFv1 first.
---

# Migration Guide: Old Logger to New fdf:: Logger

You are a coding agent tasked with migrating drivers in the Fuchsia
`src/devices` directory. Your objective is to migrate from the old driver
logging mechanisms (`DF_LOG`, `FDF_LOG`) to the new Fuchsia Driver Framework
logger (`fdf::info`, `fdf::error`, `fdf::warn`, `fdf::debug`, `fdf::trace`).

**To avoid overwhelming changes, you should process the migration ONE directory
inside `src/devices` at a time.**

## 1. Differentiate DFv1 and DFv2 Drivers (CRITICAL)
Before you start any migration, determine if the driver is DFv1 or DFv2. **We
only want to migrate DFv2 drivers.**

For a comprehensive guide on distinguishing DFv1 from DFv2 drivers (including
codebase indicators and runtime checks), see the [Driver Version Identification
Skill](../driver_version_identification/SKILL.md).

## 2. Identify Target Logs
Inside your assigned driver directory (and after confirming it is a DFv2
driver), search for old usage:
- `FDF_LOG(LEVEL, "...", arg)`
- `DF_LOG(LEVEL, "...", arg)`

Check for includes to legacy logging headers if any are present.

## 3. Update Source Code (C++)

For each file containing the old logs:

**A. Add the new header:** Replace old logging headers with the new header:
```cpp
#include <lib/driver/logging/cpp/logger.h>
```

**B. Update the Macro and Syntax:** Change the logger invocations to use the
`fdf::` namespaces:

* `FDF_LOG(INFO, ...)`  ->  `fdf::info(...)`
* `FDF_LOG(ERROR, ...)` ->  `fdf::error(...)`
* `FDF_LOG(WARNING, ...)` -> `fdf::warn(...)`
* `FDF_LOG(DEBUG, ...)` -> `fdf::debug(...)`
* `FDF_LOG(TRACE, ...)` -> `fdf::trace(...)`

**C. Convert String Formatting:** The old macros used `printf`-style formatting
(`%s`, `%d`, `%zx`). The new `fdf::` logger functions use `std::format`-style
strings (`{}`).

*Old:*
```cpp
FDF_LOG(ERROR, "Failed to send request: %s, code: %d", zx_status_get_string(status), code);
```
*New:*
```cpp
fdf::error("Failed to send request: {}, code: {}", zx_status_get_string(status), code);
```
Carefully process the string parameters replacing all `%x`, `%lu`, `%s`, etc.,
with `{}`. Pay close attention to format strings that specify width or padding
(e.g. `%02x` becomes `{:02x}` and `%-21s` becomes `{:<21}`). **IMPORTANT
CAVEAT:** Do not blindly replace `%s` or `%u` with `{}` in strings that are
passed to standard C functions like `snprintf`, `sprintf`, or macros like
`ZX_ASSERT_MSG`. These still require C-style formatting blocks. **IMPORTANT
CAVEAT (\`%x\` to \`{:x}\`):** When migrating `%x` or `%X` hex formats, pay
close attention to any zero-padding or formatting prefix:
1.  If the original format included a `0x` prefix (e.g., `0x%02x`), replacing it
    with `0x{}` will silently print decimal numbers instead of hex! You MUST use
    the inner format specifier: `0x{:x}` or drop the explicit `0x` and use
    `{:#x}`.
2.  If the original format included zero-padding or field widths like `%08x` or
    `%02X`, preserve them exactly in the new string (e.g., `{:08x}`, `{:02X}`).

**D. Formatting `zx::result` Directly:** When formatting a `zx::result` inside
an `fdf::` macro, **do not** call `.status_string()`. The Fuchsia environment
supplies a specialized `std::formatter` for `zx::result`. Pass the object
directly. *Old:* `FDF_LOG(ERROR, "Failed to send request: %s",
init_result.status_string());` *New:* `fdf::error("Failed to send request: {}",
init_result);` **IMPORTANT CAVEAT:** This specialized `std::formatter` is ONLY
available for `zx::result` and `zx::status`. Result types from FIDL calls (such
as `fidl::WireUnownedResult`) DO NOT have a specialized formatter. For FIDL
results, you MUST continue to use `.status_string()` or `.FormatDescription()`.

**E. Unused Lambda Captures (FDF_LOGL):** If you are migrating `FDF_LOGL` (which
takes an explicit logger argument), replacing it with `fdf::info` (which uses an
ambient logger) removes the usage of the logger object. If this logger was
accessed via `this` inside a lambda, the `this` capture may become unused,
causing compilation failures (`-Wunused-lambda-capture`). You MUST remove the
unused `this` from the lambda capture list.

**F. Using a Specific Logger Instance:** If the original code uses a specific
logger instance (e.g., a variable named `logger` passed as an argument or a
`logger()` method call), you SHOULD NOT use `fdf::info` etc., as those use the
ambient logger. Instead, use the `log` method on the specific logger instance:
*Old:* `FDF_LOGL(TRACE, logger, "Msg");` *New:* `logger.log(fdf::TRACE, "Msg");`

## 4. Update Build Dependencies

You must also update the driver's build configuration files (`BUILD.gn` or
`BUILD.bazel`) so they link successfully with the new logging library.

**For `BUILD.gn` files:** Locate the `deps` block for the driver
`fuchsia_cc_driver` (or the underlying `source_set`/`cc_library` the driver
wraps). Ensure you add the new SDK logging dependency:
```gn
deps += [
  "//sdk/lib/driver/logging/cpp",
]
```

**For `BUILD.bazel` files:** Add the Bazel equivalent SDK dependency:
```bazel
deps = [
    "@fuchsia_sdk//pkg/driver_logging_cpp",
]
```

## 5. Format and Validate
After making the migration changes in a directory:
- Run `fx format-code` to correct formatting issues.
- You must include the driver in the build graph before building. Run `fx set
  minimal.x64 --with //<path_to_driver_directory>` (e.g., `fx set minimal.x64
  --with //src/devices/pwm/drivers/aml-pwm-init`).
- Build the component to verify the change: `fx build`.
- Resolve any type deduction mismatch or formatting compilation errors that may
  occur from standardizing string formatting styles.
- **Do not** run `fx test` or attempt to run tests, as we currently do not have
  a device attached.

## 6. Review Diffs for Migration Accuracy
Before committing or marking the task as complete, you MUST rigorously review
the `git diff` against the original unmigrated codebase (e.g. `git diff
HEAD~1..HEAD`). Look for mistakes commonly introduced during bulk edits:
1.  **Dropped `zx_status_get_string()` calls**: `std::format` does not
    automatically stringify `zx_status_t` integer types. If the original code
    logged `zx_status_get_string(status)`, the migrated code MUST also wrap the
    arguments in `zx_status_get_string()`.
2.  **Dropped formatting specifiers**: Check if width, padding, or hex
    specifiers (e.g., `%08x`, `%p`, `%4u`) were accidentally stripped into empty
    `{}` brackets. They must be accurately translated into `std::format`
    equivalents (`{:08x}`, `{:p}`, `{:4}`).
3.  **Dropped `.status_string()` for FIDL results**: If the original code used
    `.status_string()` on a `fidl::WireResult`, ensure it was NOT mistakenly
    replaced with `.FormatDescription().c_str()` or completely dropped. It must
    remain `.status_string()`.

## 7. Commit Your Changes
After successfully migrating, formatting, and building a driver, make a local
git commit for it. The Fuchsia project commit message style looks like:

```text
[<subsystem>][<driver_name>] Migrate to fdf:: logger

Migrate old FDF_LOG statements to the new fdf::info, fdf::error
logging macros and update the associated format strings.
```
*(Example subsystem tags: `[usb]`, `[i2c]`, `[block]` depending on the
directory.)* **Note:** Omit the `Test:` line entirely from your commit message,
as we are unable to run tests at the moment. Make sure you run standard git
workflow commands using the correct `GIT_ROOT` directory.

## 8. Keep This Guide Updated
If you learn something new or discover additional edge cases while performing
this migration (e.g. specialized macros, corner cases with build rules),
**update this prompt file (`src/devices/skills/migrate_logging/SKILL.md`)** so
that future agents benefit from your learnings.
