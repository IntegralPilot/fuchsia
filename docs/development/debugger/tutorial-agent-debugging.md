# Tutorial: Debug tests using agent debugging mode

This tutorial explains the concepts and workflow of *agent debugging mode* in
`fx test` and how to manage symbol loading and breakpoint resolution when
integrating with automated scripts or IDE debuggers.

## Overview

Agent debugging mode is designed for headless automation, IDE integration (like
VS Code), and scripts. It runs `zxdb` as a background server exposing a Debug
Adapter Protocol (DAP) endpoint on a Unix domain socket at
`/tmp/fx-debug-daemon.sock`.

To start a test suite in this mode:

```posix-terminal
fx test <TEST_SUITE_NAME> --agent-debugging-mode
```

## Default behavior: weak attachment

When you run `fx test --agent-debugging-mode` without any breakpoint arguments,
the testing framework attaches `zxdb` to the target process "weakly" (using the
`attach --weak` command).

### What is weak attachment?

Weak attachment optimizes startup performance by preventing `zxdb` from
proactively querying modules and loading symbol indexes for the process.

*   **Deferred symbol loading:** `zxdb` does not load symbols upon the initial
    attach.
*   **Resolution limitations:** because the symbol index is missing, any
    `file:line` or symbolic breakpoints registered dynamically through the DAP
    UDS interface after the test starts will remain "pending" and cannot be
    verified or resolved.
*   **Autoresolution on crash:** an exception or external event (such as a Rust
    panic or a C++ assertion failure) triggers symbol loading. Once stopped,
    symbols load automatically and pending breakpoints resolve.

## Solution: proactive breakpoint installation

If you need to debug a passing path, or if you want to ensure your breakpoints
are active and verified *before* execution starts, you must specify them on the
`fx test` command line:

```posix-terminal
fx test <TEST_SUITE_NAME> --agent-debugging-mode --breakpoint <SOURCE_FILE>:<LINE>
```

### How it works

1.  The presence of the `--breakpoint` option tells the `fxtest` framework to
    attach `zxdb` *normally* (omitting the `--weak` flag).
2.  Because the attachment is normal, `zxdb` immediately loads symbols for the
    target process.
3.  `zxdb` installs and resolves the specified breakpoints upfront during the
    initialization handshake.
4.  When the test execution hits the breakpoint, the debugger suspends the
    process and sends a `"stopped"` event with `"reason": "breakpoint"` to the
    DAP client.

## Interactive debugging using fx debug cli

Once the execution stops at a breakpoint or exception, you can interact with
the debugger from a second terminal using the `fx debug cli` wrapper tool.

To connect and retrieve the current debugger state:

```posix-terminal
fx debug cli --json '{"command": "get-state"}'
```

The tool returns a JSON response listing active processes and threads:

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

To retrieve a stack trace for a specific thread, pass the `thread_id`:

```posix-terminal
fx debug cli --json '{"command": "stackTrace", "thread_id": 1}'
```

The tool returns the stack frame details:

```json
{
  "success": true,
  "body": {
    "stackFrames": [
      {
        "id": 0,
        "name": "my_test_function",
        "source": {
          "name": "main_test.cc",
          "path": "/src/main_test.cc"
        },
        "line": 42,
        "column": 1
      }
    ],
    "totalFrames": 1
  }
}
```

Once diagnostic inspection is complete, resume the execution of the thread:

```posix-terminal
fx debug cli --json '{"command": "continue", "thread_id": 1}'
```

Use `fx debug cli help` to get a list of all commands you can run.

## See also

*   [`fx debug cli` skill](/src/developer/debug/skills/fuchsia-debugger/SKILL.md)
