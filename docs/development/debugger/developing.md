# Developing and debugging zxdb

This document covers various topics to help you as you develop and debug the
zxdb debugger:

* [Run tests](#run-tests)
* [Reload `debug_agent.cm` after a new build](#reload-debug_agent)
* [Enable debug logging in `debug_agent`](#enable-debug-logging-debug_agent)
* [Enable debug logging in zxdb](#enable-debug-logging)
* [Launch zxdb in another debugger](#launch-zdb-other-debugger)
* [Debug `debug_agent` in another `debug_agent`](#debug-agent-in-another-agent)

## Run tests {:#run-tests}

To run the zxdb frontend tests:

Note: These tests run on the machine with your current Fuchsia checkout.

```posix-terminal
fx test zxdb_tests
```

To run the `debug_agent` tests:

Note: These tests run on a Fuchsia target device.

```posix-terminal
fx test debug_agent_unit_tests
fx test debug_agent_integration_tests
```

To run the end-to-end tests:

Note: These test the integration of the zxdb frontend with the `debug_agent`.

```posix-terminal
fx test --e2e zxdb_e2e_tests
```

## Reload `debug_agent.cm` after a new build {:#reload-debug_agent}

Since the `debug_agent_launcher` is a long-running process, your system does
not try to update the `debug_agent` package after the first `ffx debug connect`
invocation.

To force the system to unload `debug_agent.cm`:

```posix-terminal
ffx component stop /core/debug_agent
```

## Enable debug logging in `debug_agent` {:#enable-debug-logging-debug_agent}

To enable the debug logging of the `debug_agent`, add
`--set-severity core/debug_agent#DEBUG` to `fx log`. For example:

```posix-terminal
fx log --set-severity core/debug_agent#DEBUG --tag debug_agent --hide_metadata --pretty
```

## Enable debug logging in zxdb {:#enable-debug-logging}

To enable debug logging in zxdb:

```posix-terminal
ffx debug connect -- --debug-mode
```

## Launch zxdb in another debugger {:#launch-zdb-other-debugger}

You can have `ffx debug` launch zxdb in another debugger such as `lldb`. For
example:

```posix-terminal
ffx debug connect --debugger lldb
```

This command brings the lldb shell and you can use `run` to start zxdb.

Alternatively, instead of `lldb` you could specify another debugger such as `gdb`.
However, if you use `gdb`, you may run into some of the following issues:

  * Older versions of `gdb` may not support all DWARF 5 standards. Which may
    result some missing information such as source file listing.
  * `Ctrl-C` does not return you from zxdb to `gdb`. To stop zxdb, from another
    terminal, run:

    ```posix-terminal
    pkill -INT zxdb`
    ```

## Debug `debug_agent` in another `debug_agent` {:#debug-agent-in-another-agent}

You can attach a `debug_agent` to another `debug_agent`:

Note: This is a task to help debug issues with zxdb.

1. Run the debugger that attaches to the "to-be-debugged" `debug_agent` that you
   want to debug:

  ```posix-terminal
  ffx debug connect
  ```

1. From the zxdb console, run:

  ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
  attach debug_agent
  Waiting for process matching "debug_agent".
  Type "filter" to see the current filters.
  Attached Process 1 state=Running koid=345223 name=debug_agent.cm
  Attached Process 2 state=Running koid=345403 name=/pkg/bin/debug_agent
  ```

1. The first `debug_agent` captures the launcher and itself. You can detach the
   processes to avoid any deadlock.

   For example, to detach process 1:

   ```none {:.devsite-disable-click-to-copy}
   [zxdb] pr 1 detach
   ```

   For example, to detach process 2:

   ```none {:.devsite-disable-click-to-copy}
   [zxdb] pr 2 detach
   ```

1. Create a breakpoint on the `$main` function:

   ```none {:.devsite-disable-click-to-copy}
   [zxdb] break $main
   ```

1. From another terminal, launch another zxdb instance:

   ```posix-terminal
   ffx debug connect
   ```

1. In the initial terminal with zxdb, you should see an output like:

  ```none {:.devsite-disable-click-to-copy}
  Attached Process 1 state=Running koid=12345 name=/pkg/bin/debug_agent
  Breakpoint 1 now matching 1 addrs for $main
  🛑 process 1 on bp 1 main(int, const char**) • main.cc:101
      99
    100 int main(int argc, const char* argv[]) {
  ▶ 101   debug_agent::CommandLineOptions options;
    102   cmdline::Status status = ParseCommandLine(argc, argv, &options);
    103   if (status.has_error()) {
  ```

  You now have two instances of zxdb running.
