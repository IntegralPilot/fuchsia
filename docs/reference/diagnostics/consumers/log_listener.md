# log_listener CLI

The `log_listener` binary is a utility that is included in `eng` products by
default.

The main tool to read logs from a Fuchsia device is [`ffx log`][ffx-log]. However,
for cases when you don't have network access, only have a serial console working,
you are working in a `bringup` build, etc. there is a binary on Fuchsia devices
that can be used: `log_listener`.

For a list of the possible arguments and options that `log_listener` accepts,
see [`ffx log`][ffx-log] as both `ffx log` and `log_listener` share the same CLI.

[ffx-log]: /reference/tools/sdk/ffx.md#ffx_log
