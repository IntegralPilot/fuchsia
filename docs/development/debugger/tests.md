# Debug tests using zxdb

This page provides details and examples related to using the Fuchsia
debugger (`zxdb`) with the [`fx test`][fx-test] command.

* [Starting a zxdb session from fx test](#entering-the-debugger-from-fx-test)
* [Executing test cases in parallel](#executing-test-cases-in-parallel)
* [Closing the debugger](#closing-the-debugger)

Additionally, you can see the [Tutorial: Debug tests using zxdb][zxdb-testing-tutorial]
for more information on using zxdb with tests.

## Starting a zxdb session from fx test {:#entering-the-debugger-from-fx-test}

The `fx test` command supports the `--break-on-failure` and `--breakpoint`
flags, which allow you to debug tests using `zxdb`. If your test uses a
compatible test runner (that is, gTest, gUnit, or Rust), adding the
`--break-on-failure` flag causes test failures to pause test execution
and enter the `zxdb` debug environment, for example:

```posix-terminal
fx test --break-on-failure rust_crasher_test.cm
```

The command returns an output like:

```none {:.devsite-disable-click-to-copy}
<...fx test startup...>

Running 1 tests

Starting: fuchsia-pkg://fuchsia.com/crasher_test#meta/rust_crasher_test.cm
Command: fx ffx test run --max-severity-logs WARN --break-on-failure fuchsia-pkg://fuchsia.com/crasher_test?hash=1cceb326c127e245f0052367142aee001f82a73c6f33091fe7999d43a94b1b34#meta/rust_crasher_test.cm

Status: [duration: 13.3s]  [tasks: 3 running, 15/18 complete]
  Running 1 tests                      [                                                                                                     ]           0.0%
  👋 zxdb is loading symbols to debug test failure in rust_crasher_test.cm, please wait.
  ⚠️  test failure in rust_crasher_test.cm, type `frame` or `help` to get started.
    84     #[test]
    85     fn test_should_fail() {
  ▶ 86         assert_eq!(0, 1);
    87     }
    88 }
  🛑 process 1 rust_crasher_bin_test::tests::test_should_fail() • main.rs:86
[zxdb]
```

Since you have started a zxdb session, you can now use most of the regular
`zxdb` workflows. However, in this example the thread is already in a fatal
exception, typical execution commands such as `step`, `next`, and `until` are
not available. Inspection commands such as `print`, `frame`, and `backtrace`
are available for the duration of the debugging session.

## Executing test cases in parallel {:#executing-test-cases-in-parallel}

`zxdb` is designed to handle multi-process debugging. You can inspect the
current attached processes and their execution states with the `process` noun
or with the `status` command. The currently "active" process is marked with
a "▶" sign.

For more detailed information, see [Interaction model][interaction-model] (or
use the `help` command).

Depending on the options that you provide to `fx test` or the test runner
default configurations, multiple test cases may fail in parallel. Supported
test runners all spawn an individual process for each test case, and the default
configuration may allow for multiple test processes to be running at the same
time.

When running with `fx test`, `zxdb` attaches to _all_ processes in your
test's realm. A test case failure _only_ stops that particular process.

Parallel execution only stops if and only if the number of test case
failures is equal to the number of allowed parallel test cases. Once any
process is detached from `zxdb`, another test case process begins immediately.

## Closing the debugger {:#closing-the-debugger}

After inspecting your test failure, you can resume the test execution by
detaching from your test process, for example, by using `kill`, `detach`, or
`continue`.

As mentioned in
[Executing test cases in parallel](#executing-test-cases-in-parallel), multiple
test cases may fail in parallel. If you do not explicitly detach from all
attached processes, `zxdb` remains in the foreground. You can see all attached
processes using the `process` noun.

You can also use certain commands to detach from all processes (for example,
`quit`, `detach *`, and `ctrl+d`) and resume execution of the test suite
immediately.

<!-- Reference links -->

[fx-test]: /docs/reference/testing/fx-test.md
[interaction-model]: /docs/development/debugger/commands.md
[zxdb-testing-tutorial]: /docs/development/debugger/tutorial-tests.md