# Recording Logs

There are a few ways that a Fuchsia component generates log messages:

* The `fuchsia.logger.LogSink` protocol.
* The kernel's `debuglog`.
* A combination of the above, routed through `stdout` or `stderr`.

## `LogSink`/syslog

Components that want to generate log messages should connect to the
[`fuchsia.logger.LogSink`] protocol and then handle the
[`fuchsia.logger.LogSink/OnInit`] event. The [`fuchsia.logger.LogSink`] protocol
must be [`use`d in the component manifest][syslog-use-shard]. This protocol is
routed as part of the `diagnostics` dictionary. A component must have the
`fuchsia.logger.LogSink` protocol routed to it directly so it can use it form
`parent`, or more commonly, have the full `diagnostics` dictionary routed to it,
in which case it can use the `fuchsia.logger.LogSink` protocol from
`parent/diagnostics`.

The `OnInit` event includes an [`IOBuffer`] which is used to actually send log
messages. If the [`IOBuffer`] is full, logs will be dropped. If there are any
dropped log messages on the writer side, these are counted and that count is
sent in the next successful message, after which the counter is reset. `ffx log`
[prints a warning] when it's aware of dropped messages. The `LogSink` server
must ensure the [`IOBuffer`] has sufficient space to prevent messages being
dropped on the writer-side.

Different languages use different mechanisms to connect to `LogSink`:

* [C++ logging]
* [Go logging]
* [Rust logging]

## debuglog handles

The kernel allows components to create `debuglog` handles from the root
resource. Each handle allows its owner to write messages into the
[kernel's shared ring buffer]. Each message has a limit of
`ZX_LOG_RECORD_DATA_MAX` bytes, with content in excess being truncated.

In addition to being [bindable to file descriptors], `debuglog` handles can
be passed to the [`debuglog_write`] and [`debuglog_read`] syscalls.

If a component needs to send its standard streams to the `debuglog`, it must
have the [`fuchsia.boot.WriteOnlyLog`] routed to it.

Most logs written to `debuglog` handles are written through stdio forwarding.

## Standard streams: `stdout` and `stderr`

While these concepts may be familiar to you from other operating systems, their
use in Fuchsia can be complicated to follow because they are routed differently
in different parts of the system.

### Drivers

Note: For more information about driver logging, see
[Driver logging][driver-logging-docs].

Drivers use the `LogSink` protocol to log, but do so through the use of their
own library. This library provides a wrapper around the `syslog` library, so
that each driver has its own logger.

In addition, `driver_manager` binds `stdout` and `stderr` to `debuglog`. This
allows `driver_manager` to output critical information to the `debuglog`, or to
fallback to the `debuglog`.

### Components

By default, [components] only have their `stdout` and `stderr` streams captured
if they have access to a `LogSink` protocol for forwarding. For more information,
see the ELF runner section on [forwarding stdout and stderr streams].

## Forwarding `klog` to `syslog`

The Archivist continually reads from the `klog` and forwards those messages to
the `syslog` log. Messages from the `klog` can be dropped by the pipeline if
they are rolled out of the `klog` buffer before the Archivist reads them into
`syslog`.

All kernel log messages are sent to the system log with `INFO` severity because
the `debuglog` syscalls do not have a way to express the severity of a message.

[`fuchsia.logger.LogSink/OnInit`]: https://fuchsia.dev/reference/fidl/fuchsia.logger#LogSink.OnInit
[`fuchsia.logger.LogSink`]: https://fuchsia.dev/reference/fidl/fuchsia.logger#LogSink
[syslog-use-shard]: /sdk/lib/syslog/use.shard.cml
[`IOBuffer`]: https://fuchsia.dev/reference/syscalls#io_buffers
[prints a warning]: /src/diagnostics/log_listener/src/main.rs?l=918&drc=3a02d1922c0519b4c7d639879ec0503de9c79f0c
[C++ logging]: /docs/development/languages/c-cpp/logging.md
[Go logging]: /docs/development/languages/go/logging.md
[Rust logging]: /docs/development/languages/rust/logging.md
[kernel's shared ring buffer]: /zircon/kernel/lib/debuglog/debuglog.cc?l=37&drc=1bdbf8a4e6f758c3b1782dee352071cc592ca3ab
[bindable to file descriptors]: /sdk/lib/fdio/include/lib/fdio/fdio.h?l=36&drc=1bdbf8a4e6f758c3b1782dee352071cc592ca3ab
[`debuglog_write`]: /reference/syscalls/debuglog_write.md
[`debuglog_read`]: /reference/syscalls/debuglog_read.md
[kernel params]: /docs/reference/kernel/kernel_cmdline.md#drivernamelogflags
[`fuchsia.sys/LaunchInfo`]: https://fuchsia.dev/reference/fidl/fuchsia.sys#LaunchInfo
[`stdout-to-debuglog`]: /src/sys/lib/stdout-to-debuglog
[`fuchsia.boot.WriteOnlyLog`]: https://fuchsia.dev/reference/fidl/fuchsia.boot#WriteOnlyLog
[`ddk/debug.h`]: /src/lib/ddk/include/ddk/debug.h
[components]: /docs/concepts/components/v2/introduction.md
[ELF]: /docs/concepts/components/v2/elf_runner.md
[forwarding stdout and stderr streams]: /docs/concepts/components/v2/elf_runner.md#forwarding_stdout_and_stderr_streams
[driver-logging-docs]: /docs/development/drivers/diagnostics/logging.md
