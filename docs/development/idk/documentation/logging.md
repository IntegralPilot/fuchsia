# Logging

Logs play an important part in diagnostics in Fuchsia as they describe human
readable events that have happened on a system.

When working with the IDK, you can publish logs with the `syslog` API which is
available for C in `//pkg/syslog` and C++ in `//pkg/syslog_cpp`.

When you work with the IDK, one of the main tools for working with logs is
[`ffx log`][ffx-log-ref] to retrieve logs from a Fuchsia device.

To learn more about logging on Fuchsia, see [Logging on Fuchsia][logs-concepts].

[ffx-log-ref]: /reference/tools/sdk/ffx.md#ffx_log
[logs-concepts]: /docs/concepts/components/diagnostics/README.md
