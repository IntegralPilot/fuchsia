# `log-command`

`log-command` utility library for parsing log_listener
and ffx log command line arguments.

## Building

This project should be automatically included in builds.

## Using

`log-command` is consumed from ffx log and log_listener.

## Testing

unit tests for `log-command` are available in the
`log-command-test` package:

```
$ fx test log-command-test
```

You'll need to include `//src/diagnostics/lib/log-command:tests` in your
build, either by using `fx args` to put it under `universe_package_labels`, or
by `fx set [....] --with //src/diagnostics/lib/log-command:tests`.
