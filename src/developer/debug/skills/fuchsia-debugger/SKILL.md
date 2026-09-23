---
name: fuchsia-debugger
description: Use fx debug cli to debug test failures.
---

# Overview

`fx debug cli` is a machine-readable, agent-friendly interface to `zxdb`. It
uses the Debug Adapter Protocol (DAP) to enable turn-based interactions that
agents are already accustomed to and possess knowledge of. Unlike traditional
console-based debuggers, `fx debug cli` exposes a stateless command-line
frontend that issues request-response style commands to the backend, allowing
agents to send and receive structured JSON as input and output.

`fx debug cli` integrates with `fx test` to allow automatic pausing during test
failures, which allows Agents to inspect the state and diagnose the failure.

---

## Testing Workflow

This mode delegates the management of the debugging session to the testing
framework. The framework automatically starts the `zxdb-daemon` process and
connects to the active target.

### Step 1: Start the Test in the Background
Run `fx test` with the `--agent-debugging-mode` flag. Execute this command
asynchronously in the background so you can proceed with other work while
observing the output:

```bash
fx test <test_suite_name> --agent-debugging-mode
```

*Note: This automatically starts the `zxdb-daemon` background process, which
begins listening for messages sent via `fx debug cli`.*

### Step 2: Poll for Target Transitions
Begin polling the active daemon process for events. Start with `last_seen_seq:
0` and increment the sequence number as events are received:

```bash
fx debug cli --json '{"command": "wait-for-event", "last_seen_seq": 0, "timeout": 10}'
```

> [!NOTE]
> **Socket Startup Delay**: The background daemon may take 1-2 seconds to
> initialize and bind the listening socket after `fx test` begins. If the
> initial command fails with `Daemon socket not found`, wait briefly and retry.

#### Example `wait-for-event` Response (Test Failure / Breakpoint):
```json
{
  "success": true,
  "events": [
    {
      "seq": 1,
      "event": "stopped",
      "body": {
        "threadId": 1,
        "reason": "breakpoint"
      }
    }
  ]
}
```

> [!WARNING]
> **Thread ID Key Casing Mapping Pitfall**:
> Output events returned by `wait-for-event` use **camelCase** keys (e.g.
> `"threadId": 1`). However, subsequent request commands (such as
> `stack-trace`, `continue`, or `pause`) strictly require **snake_case**
> parameters (e.g. `"thread_id": 1`).
> Always map `"threadId"` to `"thread_id"` when programmatically invoking
> requests.

Simultaneously, monitor the output and status of the background `fx test` task.

### Step 3: Handle Execution Scenarios

Depending on the status of the test run, handle one of the following scenarios:

#### Scenario A: Test Failure (Debugger Suspended)
If `wait-for-event` returns a `"stopped"` event, it indicates a test failure or
breakpoint hit. Perform diagnostics:

1.  **Query Session State:** Retrieve all threads and processes active in the
    session to identify the failing thread:
    ```bash
    fx debug cli --json '{"command": "get-state"}'
    ```
    Example Response:
    ```json
    {
      "success": true,
      "body": {
        "threads": [
          { "id": 1, "name": "initial-thread" }
        ],
        "processes": { "12345": "my_test_binary" }
      }
    }
    ```
    *Note: In the `get-state` thread list representation, the key is `"id"`. Map
    this value to the `"thread_id"` parameter in subsequent requests.*

2.  **Retrieve Stack Trace:** Get the stack trace for a suspended thread
    (`thread_id`) or across all threads in a process (`pid`):
    ```bash
    # Thread-level stack trace
    fx debug cli --json '{"command": "stack-trace", "thread_id": 1}'

    # Process-level stack trace (all threads in "pid")
    fx debug cli --json '{"command": "stack-trace", "pid": 12345}'
    ```
    Example Thread Response:
    ```json
    {
      "success": true,
      "body": {
        "thread_id": 1,
        "stack_frames": [
          {
            "frame_index": 0,
            "name": "my_test_function",
            "source": {
              "name": "main_test.cc",
              "path": "/src/main_test.cc"
            },
            "line": 42,
            "column": 1
          }
        ],
        "total_frames": 1
      }
    }
    ```
    Example Process Response:
    ```json
    {
      "success": true,
      "body": {
        "process_id": 12345,
        "stacks": [
          {
            "thread_id": 1,
            "stack_frames": [
              {
                "frame_index": 0,
                "name": "my_test_function",
                "source": {
                  "name": "main_test.cc",
                  "path": "/src/main_test.cc"
                },
                "line": 42,
                "column": 1
              }
            ],
            "total_frames": 1
          },
          {
            "thread_id": 2,
            "stack_frames": [
              {
                "frame_index": 0,
                "name": "worker_thread_entry",
                "source": {
                  "name": "worker.cc",
                  "path": "/src/worker.cc"
                },
                "line": 105,
                "column": 5
              }
            ],
            "total_frames": 1
          }
        ]
      }
    }
    ```

3.  **Inspect Local Variables:** Retrieve variables and arguments within a
    specific stack frame:
    ```bash
    fx debug cli --json '{"command": "variables", "thread_id": 1, "frame_index": 0}'
    ```

4.  **Evaluate Dynamic Expressions / Commands:** Run dynamic expression
    evaluation or debugger commands within a stack frame. The underlying
    debugger handles `evaluate` requests in the `"repl"` context, executing the
    expression directly as a zxdb console command line.

    Note that `evaluate` requires the target thread to be stopped prior to
    invocation. If the thread is running, the request immediately returns an
    error (`success: false`). Ensure the thread is suspended via a breakpoint or
    `pause` command before evaluating.

    To evaluate an expression or print a variable, prefix the expression with
    the debugger's print command:
    ```bash
    fx debug cli --json '{"command": "evaluate", "thread_id": 1, "frame_index": 0, "expression": "print my_var"}'
    ```

    Example Response:
    ```json
    {
      "success": true,
      "body": {
        "result": "42",
        "type": "int"
      }
    }
    ```

    You can also execute arbitrary zxdb console commands such as symbol
    inspection:
    ```bash
    fx debug cli --json '{"command": "evaluate", "thread_id": 1, "frame_index": 0, "expression": "sym-info my_var"}'
    ```

5.  **Teardown Session:** Once diagnostics are complete, terminate the debugging
    session. This detaches from targets and lets the background test runner exit
    cleanly:
    ```bash
    fx debug cli --json '{"command": "stop"}'
    ```

#### Scenario B: Clean Pass (All Tests Succeed)
If all tests pass cleanly, the background `fx test` task will complete
successfully, exiting with status code `0` and summarizing `FAILED: 0` in its
output.

* **Edge Case: Poll Connection Failures**: Since `fx test` automatically reaps
  the background daemon process on a clean exit, any active, blocking
  `wait-for-event` command will terminate abruptly with a connection error
  (e.g., connection refused or closed socket).
* **Action**: If `wait-for-event` returns a connection failure, check the status
  of the background `fx test` task. If it completed successfully (exit code 0),
  treat this as a successful clean pass, print a success message, and exit. **No
  manual `"stop"` command or teardown is required.**

#### Scenario C: Proactive Breakpoint Installation (Non-Failing Paths)
By default, the testing framework attaches to targets weakly (`attach --weak`).
In this state, symbols are not loaded upfront, and dynamic `break` requests will
remain pending and unresolved until a crash occurs.

To debug a passing path or install breakpoints before test execution begins:

1.  Start the test with the `--breakpoint` option to force a normal attach with
    immediate symbol loading (paths must be fully qualified from the workspace
    root):
    ```bash
    fx test <test_name> --agent-debugging-mode --breakpoint <source_file>:<line>
    ```
2.  Poll for the breakpoint hit stopped event via `wait-for-event`.
3.  Once suspended, perform diagnostics and resume execution using `continue`.

---

## JSON Request Reference

All commands are sent as serialized JSON payloads to `fx debug cli --json
'<payload>'`.

| Action | Command Input Payload |
|---|---|
| **Wait for Event** | `{"command": "wait-for-event", "last_seen_seq": <seq>, "timeout": <secs>}` |
| **Get State** | `{"command": "get-state"}` *(Returns active threads, processes, and breakpoints)* |
| **List Threads** | `{"command": "threads"}` |
| **Set/Delete Breakpoint** | `{"command": "break", "file": "<workspace_root_path>", "line": <line_num>, "delete": <optional_bool>}` *(To delete, specify matching file and line)* |
| **Attach Process** | `{"command": "attach", "filter": "<name_or_pid>"}` |
| **Detach Process** | `{"command": "detach", "pid": <pid>}` or `{"command": "detach", "all": true}` |
| **Get Stack Trace** | `{"command": "stack-trace", "thread_id": <thread_id>}` or `{"command": "stack-trace", "pid": <pid>, "raw": <optional_bool>}` *(CLI: `stack-trace -t <id>` or `stack-trace -p <pid>`)* |
| **List Variables** | `{"command": "variables", "thread_id": <thread_id>, "frame_index": <frame_index>}` *(CLI: `variables -t <id> --frame-index <idx>`)* |
| **Evaluate Expression** | `{"command": "evaluate", "thread_id": <thread_id>, "frame_index": <frame_index>, "expression": "<expr>", "start": <start>, "count": <count>}` *(CLI: `evaluate -t <id> --frame-index <idx> <expr>`)* |
| **Step In** | `{"command": "step-in", "thread_id": <thread_id>}` *(CLI: `step-in <id>`)* |
| **Step Over (Next)** | `{"command": "next", "thread_id": <thread_id>}` *(CLI: `next <id>`)* |
| **Step Out (Finish)** | `{"command": "finish", "thread_id": <thread_id>}` *(CLI: `finish <id>`)* |
| **Continue Thread** | `{"command": "continue", "thread_id": <thread_id>}` *(CLI: `continue <id>`)* |
| **Pause Thread / Process** | `{"command": "pause", "thread_id": <thread_id>}` or `{"command": "pause", "pid": <pid>}` *(CLI: `pause -t <id>` or `pause -p <pid>`)* |
| **Stop Session** | `{"command": "stop"}` |

### Global Parameters
* **Event History Pruning (`ack_seq`)**: Any command payload can include
  `"ack_seq": <seq>` (e.g., `{"command": "get-state", "ack_seq": 5}`). The
  daemon will prune all event history up to the acknowledged sequence to
  optimize memory.

### Performance & Blocking
* **Smart Blocking**: Commands like `pause`, `stack-trace`, and `wait-for-event`
  are blocking operations and may take up to 10 seconds depending on the
  target's execution state. Unlike those commands, `evaluate` is non-blocking
  and fails immediately if the target thread is not already stopped.
