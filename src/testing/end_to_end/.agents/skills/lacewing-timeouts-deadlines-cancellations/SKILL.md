---
name: lacewing-timeouts-deadlines-cancellations
description: Guidelines and patterns for handling timeouts, deadlines, cancellations, and exceptions in Fuchsia Lacewing tests.
---

# Lacewing Timeouts, Deadlines, and Cancellations Skill

This skill provides guidelines and patterns for handling test timeouts, operation deadlines, cancellations, and custom exceptions in Fuchsia Lacewing host-driven end-to-end tests.

## Workflow & Guidelines

### 1. Test Timeouts (Infrastructure Level)
Timeouts are configured at the GN/test specification level:
*   `timeout_secs` specifies the total allowed duration for the test.
*   `cleanup_period_secs` reserves a time window at the end of execution to send `SIGTERM` to the test, allowing graceful teardown and saving logs.
*   If a test target requires a custom `timeout_secs`, the target must be added to the allowlist in `//build/testing/timeouts/BUILD.gn`.

### 2. Operation Deadlines (Python Code Level)
Within python test code, use the custom `Deadline` class (defined in `honeydew.utils.deadline`) rather than standard timezone-naive `datetime`.
*   Retrieve/calculate the global test deadline using the runner's timeout config if available.
*   For all wait and retry operations, derive a subdeadline from the global test deadline using `subdeadline_with_timeout(duration)`.
*   Pass the derived deadline to Honeydew's control flow helpers (e.g. `retry_until_deadline()`, `repeat_until_deadline()`). This ensures the operation fails gracefully before the overall runner timeout terminates the process.

```python
# Derive a subdeadline no longer than 60s, or the remaining global test time
op_deadline = self.test_deadline.subdeadline_with_timeout(timedelta(seconds=60))

# Execute retry logic bound by the subdeadline
await retry_until_deadline(
    self.dut.some_operation,
    deadline=op_deadline,
)
```

### 3. Graceful Cancellations & Cleanup
When a test is cancelled (e.g., via `SIGTERM` from the runner or `Ctrl+C`):
*   Mobly catches the signal and runs `teardown_test()`, `teardown_class()`, and destroys all registered controllers.
*   Tests must register cleanup actions (such as restoring device states, deleting temp files) in `teardown_test()` or Mobly cleanups to ensure they run during this window.
*   Never catch `BaseException` or swallow `asyncio.CancelledError`. Always use `except Exception:` to catch standard runtime errors, allowing cancellations to propagate.

### 4. Custom Exceptions
*   **Honeydew APIs**: Custom exceptions raised by Honeydew device APIs, transports, or affordances must be defined in `//src/testing/end_to_end/honeydew/honeydew/errors.py` by inheriting from `HoneydewError`.
*   **Test Cases**: Test-specific assertions or conditions can raise standard Python exceptions or Mobly signals (such as `mobly.signals.TestFailure` or `mobly.signals.TestAbortSignal`).
