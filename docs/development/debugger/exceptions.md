# Work with exceptions

[Exceptions in Zircon](/docs/concepts/kernel/exceptions.md) are handled in
several phases:

1. zxdb is notified of an exception which is called the "first chance".
   At this stage, zxdb may handle this exception. For example, if the debugger
   continues after a single-step or breakpoint exception, the case exception
   processing stops.

1. zxdb can choose to forward the exception to the normal handlers as if a
   debugger were not present. Then, the component itself may resolve the
   exception.

1. If at this stage, the exception is still unhandled, zxdb gets the exception
   again as a "second chance" exception.

## Forwarding exceptions

If an exception is found and execution is continued through `continue`, `step`,
`next`, etc..., zxdb re-runs the excepting instruction. Normally, this causes
the same exception again and the component execution cannot make progress.

This behavior can cause problems with `gtest`, where an exception is
the expected result of the test. This is called a "death test". However, the
test harness expects to catch this exception and continue with the test.

To forward the exception to the component, the exception needs to be explicitly
forwarded. You can forward the exception with `--forward` to `continue` command:

For example, upon the expected crash, a death test reports:

```none {:.devsite-disable-click-to-copy}
══════════════════════════
 Invalid opcode exception
══════════════════════════
 Process 2 (koid=57368) thread 7 (koid=57563)
 Faulting instruction: 0x4356104fba24

🛑 Process 2 Thread 7 scudo::die() • fuchsia.cpp:28
   26 uptr getPageSize() { return PAGE_SIZE; }
   27
 ▶ 28 void NORETURN die() { __builtin_trap(); }
   29
   30 // We zero-initialize the Extra parameter of map(), make sure this is consistent
```

You can then continue with the test by using `--forward`:

Note: The `--forward` option can also be expressed with `-f`.

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
continue --forward
```

## Automatically forwarding certain types of exceptions

zxdb can automatically forward certain exception types to the component and only
handle them as second chance exceptions. By default, only page faults are
included.

The debugger's `second-chance-exception` setting contains the list of exceptions
that are handled only as second-chance by default. This setting holds a list of
exception type abbreviations:

 * `gen`: General
 * `pf`: Page faults
 * `ui`: Undefined instruction
 * `ua`: Unaligned access

See the debugger's `help get` and `help set` for more details on dealing with list settings. Some
examples:

To list the current values of `second-chance-exceptions`:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
get second-chance-exceptions
```

To add the general exception type to `second-chance-exceptions`:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
set second-chance-exceptions += gen
```

To remove the page fault type from the list of `second-chance-exceptions`:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
set second-chance-exceptions -= pf
```
