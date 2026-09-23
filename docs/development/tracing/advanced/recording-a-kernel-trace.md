# Recording a kernel trace

Caution: In most cases you should use `ffx trace` to record traces, see
[Record traces for performance analysis][ffx-trace-guide]. However, this guide
is useful if you don't have access to the `ffx` tool such as when you
want to do a kernel trace in bringup builds.

The kernel writes records to an internal buffer to capture traces of various
actions. These traces can then be retrieved, printed, and viewed in a tool such
as [Perfetto][`ui.perfetto.dev`].

## Kernel trace format

The kernel trace format uses [FXT](/docs/reference/tracing/trace-format.md).

## Trace buffer size

The size of the trace buffer is fixed at boot time and is controlled by
the [`ktrace.bufsize`] kernel command-line parameter. Its value is the
buffer size in megabytes. The default is 32MB.

## Control what to trace

You can control what to trace through the kernel command-line parameter
`ktrace.grpmask`. The value is specified as `0xNNN` and is a bitmask of tracing
groups to enable. See the **KTRACE_GRP_** values in
`system/ulib/zircon-internal/include/lib/zircon-internal/ktrace.h`. By default,
the value is `0xfff` which traces everything.

You can also dynamically control what to trace with the `ktrace` command-line
utility.

## `ktrace` command-line utility

You can control kernel tracing with the `ktrace` command-line utility.

Note: For more information about `ktrace`, see
[Zircon Kernel Command Line Options][kernel-cmd-ref].

Use `ktrace --help` to see all of the available commands:

```posix-terminal
ktrace --help
```

You should see an output like:

```none {:.devsite-disable-click-to-copy}
Usage: ktrace [options] <control>
Where <control> is one of:
  start <group_mask>  - start tracing
  stop                - stop tracing
  rewind              - rewind trace buffer
  written             - print bytes written to trace buffer
    Note: This value doesn't reset on "rewind". Instead, the rewind
    takes effect on the next "start".
  save <path>         - save contents of trace buffer to <path>

Options:
  --help  - Duh.
```

## View a  kernel trace

You can view a trace with [`ui.perfetto.dev`]. Before you can view a trace, you
need to capture a trace.

1. Start the trace on the target:

  ```posix-terminal
  ktrace start {{ '<var>0xfff</var>' }}
  ... do something ...

1. Stop the trace:

  ```posix-terminal
  ktrace stop
  ```

1. Save the trace to `/tmp/save.ktrace`:

  ```posix-terminal
  ktrace save /tmp/save.ktrace
  ```

1. Use `fx cp` to copy the file to your development host:

  ```none{: .devsite-terminal data-terminal-prefix="host$" }
  fx cp --to-host /tmp/save.ktrace save.fxt
  ```

1. View the trace file in [`ui.perfetto.dev`].

## Use with Fuchsia Tracing

Fuchsia's tracing system supports collecting kernel trace records through the
`ktrace_provider` trace provider. For more information about Fuchsia's tracing
system, see [Fuchsia tracing system][tracing-concepts].

[`/zircon/system/ulib/zircon-internal/include/lib/zircon-internal/ktrace.h`]: /zircon/system/ulib/zircon-internal/include/lib/zircon-internal/ktrace.h
[kernel-cmd-ref]: /docs/reference/kernel/kernel_cmdline.md
[tracing-concepts]: /docs/concepts/kernel/tracing-system.md
[ffx-trace-guide]: /docs/development/tools/ffx/workflows/record-traces.md
[`ktrace.bufsize`]: /docs/gen/boot-options.md#ktracebufsizeuint32_t
[`ui.perfetto.dev`]: https://ui.perfetto.dev/
