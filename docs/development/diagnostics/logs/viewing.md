# Viewing logs

Logs are primarily consumed in either an interactive [online](#online) context
with a live Fuchsia target device or in an [offline](#offline) context with
logs collected from past execution of a device.

All logs have a timestamp attached, which is the time since booting the device
and recording the log message. A reader may receive messages in a different
order than indicated by their timestamps.

There are two buffers, volatile and persistent, that store logs. For more
information about where logs are stored on a device, see
[Storage][logs-storage-concepts].

## Online {#online}

The Logs from Fuchsia can be viewed interactively through `ffx log`.

Note: For more information on using `ffx log`, see [`ffx log`][ffx-log-ref].

```posix-terminal
ffx log
```

To view specifics logs, you can use the options such as `--filter`, `--tag`,
`--moniker`. For example, you can use `--filter` to see all logs tagged with
`network`:

```posix-terminal
ffx log --filter network
```

If you work on things such as bringup or don't have network access to the device,
you can connect to a Fuchsia device through a serial console. In those
cases, you can use [`log_listener`](#log_listener).

### log_listener

Note: For more information on the `log_listener` CLI, see
[`log_listener`][log-listener-ref].

During development, you can use [`ffx log`][ffx-log-ref] to see all logs
including those [forwarded from the klog].

For scenarios when you only have access to the serial console, no network access,
or you are simply working on a `bringup` build, it can be useful to run
[`log_listener`] in the serial console. This command uses the same CLI as
`ffx log`. If you are working with `log_listener`, any example that uses
`ffx log` can be replaced with `log_listener`.

`ffx log` receives logs through the [`fuchsia.diagnostics.ArchiveAccessor`]
protocol.

### Format

Logs are printed in this format:

```none
[seconds since boot][pid][tid][tags] SEVERITY: message
```

The timestamp is relative to the time since boot and it is formatted with
microsecond granularity.

Note: You can use the `--clock` option of [`ffx log`][ffx-log-ref] to change
the timestamp.

If the message `Something happened` is written at `WARN` severity level by
the `my-component` component from process `1902`, thread `1904` and time
`278.14`, the output looks like:

```none
[278.14][1902][1904][my-component] WARN: Something happened
```

The `ffx log` command has several options such as `--show-metadata` and
`--show-full-moniker` to display more metadata about a log. If you have a
Fuchsia device running, you can run `ffx log --help` to see the options for
modifying the output format.

See [`ffx log`][ffx-log-ref] for more information.

### Dynamically setting minimum log severity

By default, components that integrate with [`fuchsia.logger.LogSink`] through
Fuchsia's logging libraries, support the configuration of their minimum log
severity at runtime.

You can do this by passing `--set-severity` to `ffx log`. The `--set-severity`
option accepts a string argument in the format of `<component_query>#<SEVERITY>`,
where `<component_query>` is a string that will fuzzy match a component moniker
or URL and `<severity>` is one of `TRACE`, `DEBUG`,`INFO`,`WARN`,`ERROR`,`FATAL`.

For example, if you run:

```posix-terminal
ffx log --set-severity netstack#DEBUG
```

The `core/network/netstack` component starts emitting `DEBUG` logs and the
`ffx log` output prints a log stream containing `DEBUG` logs for `netstack`.
When the command is stopped (for example through Ctrl-C) the component goes
back to emitting logs with its default minimum severity (typically `INFO`).

This functionality is also supported for tests. For more information, see
[Set the minimum log severity][test-dynamic-severity].

## `fx test` {#fx-test}

Under the hood, `fx test` calls `run-test-suite`, which collects isolated
`stdout`, `stderr`, and `LogSink` connections from test components, printing
the output inline and preventing them showing up in the global log buffers.

For tests that are not yet components no interception of logs is performed.

For more information about hermetic tests, see
[Hermetic integration tests][hermetic-integration-tests].

## Kernel logs

The `klog` or [debuglog] is [printed over the kernel console][kernel-console]
and serial console.

[`netsvc`] forwards the `klog` over UDP, which is the printed output when you run
`fx klog`. If you cannot run `ffx log` or establish an SSH connection to a
Fuchsia device fails, you can run `fx klog` in a background terminal to capture
kernel logs.

Alternatively, you can also use [`dlog`] from a device shell to dump the kernel
debug logs.

### Format

Note: You can also run `ffx log --kernel` to view the kernel logs.

Kernel logs from the serial logs are printed in this format:

```none
[timestamp] pid:tid> message
```

The timestamp is based on the time since the device booted. It is formatted with 5 digits
(leading zeroes) for seconds and three digits for milliseconds (trailing zeroes).

Process and thread KOIDs are written with 5 digits each (leading zeroes).

If the message `Something happened` is written from process `1902`, thread
`1904`, and time `278.14`, the output looks like:

```none
[00278.140] 01902:01904> Something happened
```

You can use the `fx pretty_serial` command to reduce the metadata printed by
klog and color code log lines by severity. With `fx pretty_serial`, some
metadata, such as PID, TID, filenames, is hidden, while other metadata,
such as timestamp and severity, are trimmed down.

Serial output should be piped in from the emulator or from other sources.

For example, to view the pretty output of the kernel logs from an emulator:

```posix-terminal
ffx emu start --console | ffx debug symbolize
```

For example, if the message `Something happened` is printed to klog at
`WARN` severity level by the `my-component` component at time `278.14`, the
pretty output looks like:

```none
[278.14][my-component][W] Something happened
```

For example, if the message `Something happened` is printed to klog with
an unknown severity by an unknown component at time `278.14`, the pretty output
looks like:

```none
[278.14] Something happened
```

## Sending syslog to serial

Some logs from syslog are printed to the serial console. By
default, this includes all the components under the `bootstrap` realm such as:
drivers and `driver_manager`. Additional components may be added through the
[Diagnostics assembly configuration][assembly-diagnostics].

## Offline: CQ/CI/LUCI {#offline}

When running tests, a [Swarming] bot invokes [botanist], which collects
several output streams to be presented in the web UI. In the web UI, The "Swarming
task UI" displays the `stdout` and `stderr` of botanist.

For individual test executables, botanist uses the [testrunner] library and
collects that output separately. You can see this output after a
test fails with a link named `stdio`. Most tests that `testrunner` invokes run
`run-test-suite` through an SSH connection to the target device. This collects
the `stdout`, `stderr`, and logs from the test environment and prints them
inline.

### syslog.txt

The `syslog.txt` file contains the output from Botanist running `ffx log`
on the target device.

### infra_and_test_std_and_klog.txt

The `infra_and_test_std_and_klog.txt` log includes the `stdout` and `stderr` of
the command run by the [Swarming] task.

Normally this includes the following:

* [Botanist]'s log messages.
* Kernel log from [`netsvc`]. This is equivalent to `fx klog`.
* The `stdout`, `stderr`, and test logs of the tests that `testrunner` runs.

This aggregated log is run through the equivalent of `ffx debug symbolize`
before being uploaded and visible in the web UI.

### serial_log.txt

The `serial_log.txt` log includes serial logs from a target device.

### triage_output.txt

The `triage_output.txt` log includes the results of running the [triage tool]
on a snapshot collected from a target device.

### summary.json

The `summary.json` log includes a structured output summary of the test
execution.

### $debug details

The `$debug` details include debug logs emitted during the execution of infra's
recipe steps.

### $execution details

The `$execution` details of a recipe step, including the command run and
details about the execution environment. This log is often helpful for
reproducing the recipe step locally.

[hermetic-integration-tests]: /docs/reference/testing/what-tests-to-write.md#hermetic_integration_tests
[ffx-log-ref]: https://fuchsia.dev/reference/tools/sdk/ffx#ffx_log
[debuglog]: /docs/reference/kernel_objects/debuglog.md
[logs-storage-concepts]: /docs/concepts/components/diagnostics/README.md#log_storage
[`log_listener`]: /src/diagnostics/log_listener/README.md
[kernel-console]: /zircon/kernel/lib/debuglog/debuglog.cc
[`netsvc`]: /src/bringup/bin/netsvc/debuglog.cc
[`dlog`]: /src/bringup/bin/dlog/README.md
[botanist]: /tools/botanist/cmd/main.go
[testrunner]: /tools/testing/testrunner/lib.go
[triage tool]: /docs/development/diagnostics/triage/README.md
[Swarming]: https://chromium.googlesource.com/infra/luci/luci-py/+/HEAD/appengine/swarming/doc/README.md
[log-listener-ref]: /docs/reference/diagnostics/consumers/log_listener.md
[forwarded from the klog]: /docs/development/diagnostics/logs/recording.md#forwarding-klog-to-syslog
[`fuchsia.diagnostics.ArchiveAccessor`]: /reference/fidl/fuchsia.diagnostics#ArchiveAccessor
[assembly-diagnostics]: /reference/assembly/DiagnosticsConfig/index.md
[test-dynamic-severity]: /docs/development/testing/run_fuchsia_tests.md#set_the_minimum_log_severity
[`fuchsia.logger.LogSink`]: /reference/fidl/fuchsia.logger#LogSink