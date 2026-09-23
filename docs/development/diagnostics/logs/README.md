# Logging on Fuchsia

The Logs from Fuchsia can be viewed interactively through `ffx log`.

Note: If you work on things such as bringup or diagnose issues with the network
stack, you may connect to a Fuchsia device through a serial console. In those
cases, you can use [`log_listener`][log-listener-ref] to view logs from the
device.

As you work with logs from a Fuchsia device, you can use `ffx log` to
retrieve logs from a device:

Note: For more information on using `ffx log`, see [`ffx log`][ffx-log-ref].

```posix-terminal
ffx log
```

To view specifics logs, you can use the `--filter` option. For example, to see
all logs tagged with `network`:

```posix-terminal
ffx log --filter network
```

You may want to learn more about:

* [Understanding logs on Fuchsia][logs-concepts]
* [Configuring the `ffx log` tool][ffx-command-docs]
* [Understanding how Fuchsia components can write logs][recording-logs]
* [Understanding the details of a log][viewing-logs]

[ffx-command-docs]: /docs/development/tools/ffx/commands/log.md
[logs-concepts]: /docs/concepts/components/diagnostics/README.md#logs
[recording-logs]: /docs/development/diagnostics/logs/recording.md
[viewing-logs]: /docs/development/diagnostics/logs/viewing.md
[ffx-log-ref]: /reference/tools/sdk/ffx.md#ffx_log
[log-listener-ref]: /docs/reference/diagnostics/consumers/log_listener.md