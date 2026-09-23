# User guide for fx test

This page provides best practices, examples, and reference materials
for using the `fx test` command for running tests in
a [Fuchsia source checkout setup][fuchsia-source-checkout]
(`fuchsia.git`).

## Basic usage

To get started, simply run `fx test`:

```posix-terminal
fx test
```

This will do several things:

1. Identify tests included in your current build.
1. Select a subset of included tests based on selection criteria.
1. Rebuild and republish those tests.
1. Check that an appropriate Fuchsia device exists to run tests on.
1. In parallel, start running tests on that device and provide status output.
1. Write a log file describing the operations that occurred.

If you did not include any tests in your build, `fx test` will exit.
Try `fx set core.x64`**`--with //src/diagnostics:tests`** on your
`fx set` command line to include some tests as an example.

For more details on the current status of `fx test`, see this
[`README`][fxtest-source] page.

## Basic concepts

`fx test` is a **Test Executor**, which means it ingests a list of
available tests and is responsible for scheduling and observing
their execution. The source of this data is
[`tests.json`](tests-json-format.md).

Each test listed in `tests.json` is a **Test Suite** which may each
contain any number of **Test Cases**. That is, a Test Suite is a
single binary or Fuchsia Component, and it contains Test Cases
which are defined in a way specific to each test framework (e.g.
C++ `TEST`, Rust `#[test]`, Python `unittest.TestCase`). Enumerating
and executing on-device Test Cases is the responsibility of the
[Test Runner
Framework][trf-docs].

### Basic test selection

`fx test` supports selecting individual Test Suites using command
line options.  This allows you to include a large number of tests
in your build and then only execute a subset of those tests.

Any non-flag argument to `fx test` is a **selection** that is
fuzzy-matched against each test in the input:

```posix-terminal
fx test archivist --dry
```

Note: The examples in this section pass `--dry` to do a dry-run.
This will simply list selected tests without running them.

By default, the following fields are searched:

|Field|Description|
|---|---|
|name|The full name of the test. This is component URL for on-device tests and test binary path for host tests.|
|label|The build label for the test. For example, `//src/examples:my_test`.|
|component name|The name of the component manifest (excluding `.cm`) for on-device tests only.|
|package name|The name of the Fuchsia package for on-device tests only.|

You can select all tests below a directory in the source tree by
listing the prefix:

```posix-terminal
fx test //src/diagnostics/tests --dry
```

By default all of the above fields are matched, but you can select
specific fields using `--package` or `--component`:

```posix-terminal
fx test --package archivist_unittests --dry
```

By default, multiple selections on the command line implement an
inclusive-OR operation. Test selection supports composite AND
operations as follows:

```posix-terminal
fx test --package archivist --and unittests --dry
```

This command selects all tests where the package matches `archivist` and any field
matches `unittests`.

If you know the exact name of the test you want to execute, you may
use the `--exact` flag to select only that test:

```posix-terminal
fx test --exact fuchsia-pkg://fuchsia.com/archivist-tests#meta/archivist-unittests.cm --dry
```

If no tests match your selection, `fx test` will try to heuristically match
tests in your source checkout and suggest `fx set` arguments to include them:

```bash {:.devsite-disable-click-to-copy}
$ fx test driver-tests --dry
...
For `driver-tests`, did you mean any of the following?

driver_tools_tests (91.67% similar)
    --with //src/devices/bin/driver_tools:driver_tools_tests
driver-runner-tests (90.96% similar)
    --with //src/devices/bin/driver_manager:driver-runner-tests
driver-inspect-test (90.96% similar)
    --with //src/devices/tests/driver-inspect-test:driver-inspect-test
```

You can then add the necessary packages to your build.

### Basic test output

`fx test` stores its output in log files for later analysis. You
can view a summary of this log file in text form using the
`-pr/--previous` argument. For example, to see test logs from the
previous run:

```bash {:.devsite-disable-click-to-copy}
$ fx test -pr log
previous-log-file.json.gz:

4 tests were run

[START first_test]
...
[END first_test]
```

For a full list of options for processing previous log files, run
`fx test -pr help`.

By default this command processes the most recent log stored in
your Fuchsia output directory, but you may pass `--logpath` to
choose a specific log.

This command is resilient to corrupt or incomplete log files,
so it should still work even if you terminate the `fx test` command
running the tests.

### Basic test debugging

`fx test` integrates with `zxdb` to provide a simple and easy way to debug your
test failures, without needing to recompile anything. Pass `--break-on-failure`
to your `fx test` invocation to automatically have test failures break into the
debugger:

```none {.devsite-disable-click-to-copy}
$ fx test --break-on-failure rust_crasher_test.cm
...
⚠️  zxdb caught test failure in rust_crasher_test.cm, type `frame` to get started.
   14 LLVM_LIBC_FUNCTION(void, abort, ()) {
   15   for (;;) {
 ▶ 16     CRASH_WITH_UNIQUE_BACKTRACE();
   17     _zx_process_exit(ZX_TASK_RETCODE_EXCEPTION_KILL);
   18   }
══════════════════════════
 Invalid opcode exception
══════════════════════════
 Process 1 (koid=107752) thread 1 (koid=107754)
 Faulting instruction: 0x4159210ab797

🛑 process 1 __llvm_libc::__abort_impl__() • abort.cc:16
[zxdb] // Now you can debug why the test failed!
```

You can also use the `--breakpoint=<location>` option to set a breakpoint at a specific
location anywhere in your code. `<location>` takes standard zxdb breakpoint
syntax, typically a file and line number or a function name:

  * `--breakpoint=my_file.rs:123` sets a breakpoint on line 123 of my_file.rs.
  * `--breakpoint=some_function` sets a breakpoint on `some_function`.

Note that this option will cause your tests to run significantly slower, since
zxdb will need to load all of the symbols for your test to be able to install
the breakpoint. It is highly recommended to only use this option in addition to
`--test-filter`.

When you're finished debugging the test failure, you can type `quit`, `ctrl+d`,
or `detach *` to resume running your tests. Note, if there were multiple test
case failures, this will not pause to let you debug those tests as well. See
[debugging tests][zxdb-testing-docs] for details about how to debug multiple
test failures that occur in parallel.

You can also enable the Debug Adapter Protocol (DAP) server in `zxdb` by passing
`--enable-debug-adapter`. This allows you to connect external tools (like
zxdb-cli or VS Code) to the debugger. Note that `--enable-debug-adapter` requires
either `--break-on-failure` or `--breakpoint=<location>` to be set, and is
incompatible with `--use-existing-debugger`. You can optionally specify a port with
`--debug-adapter-port=<port>`, an open port is randomly assigned if unspecified.

## Configuration options

`fx test` is highly configurable, and a full list of options is
available at `fx test --help`.

This section describes how configuration options are specified and
what they mean. Configuration options are categorized as Utility,
Build, Test Selection, Execution, or Output Options. They may be
specified on the command line or in a configuration file.

### Configuration file

All arguments for `fx test` are set on the command line, but defaults may be set
per-user. If you place a file called `.fxtestrc` in your HOME directory, the arguments in that file will be the new defaults for future `fx test` invocations.

For example:

```
# ~/.fxtestrc
# Lines starting with "#" are comments and ignored.
# The below config roughly matches the behavior of the old Dart-based `fx test`.

# Default parallel to 1.
--parallel 1

# Disable status output.
--no-status

# Print output for tests taking longer than 2 seconds.
--slow 2
```

The above file overrides the defaults for `--parallel` and `--status`
flags, which normally default to `4` and `false` respectively. The new defaults
may still be overridden on the command line when invoking `fx test`.

### Utility options

Utility options change the overall behavior of `fx test`.

**`--dry`** performs a "dry-run." `fx test` will complete test selection, but
will then simply print the list of selected test suites rather than executing
any of them.

**`--list`** runs `fx test` in "list mode." Rather than executing
tests, this command lists all test *cases* within each test suite.
It outputs the appropriate command line to run each individual case.
Note that this does require access to a Fuchsia device or emulator
because cases are enumerated by Test Manager on device.

**`-pr/--prev/--previous COMMAND`** will process the log file from
a previous execution of `fx test`, and will print information
depending on the value of `COMMAND`. No new tests are executed.
This command respects `--logpath` to specify the log to read from.

The following `COMMAND`s are implemented:

- `log` prints the command line and output for each test recorded
in the log file.
- `path` prints the path to the most recent log file.
- `replay` will replay the previous run, using new display options.
The speed of the replay can be controlled using the `--replay-speed`
argument. Values > 1 speed up output, and values < 1 show the run
in slow motion.
- `stats` will print statistics about the previous run, including the longest
operations and a summary of execution time by category.
- `show-affected-tests` does not run tests. Instead, it prints the tests
affected by the modified files in the local repository.

- `run-affected-tests` automatically selects and runs tests affected by the
modified files in the local repository.
- `help` prints a summary of available commands.

### Build options

`fx test` builds and updates selected tests by default. This is
useful when running `fx -i test`, which will detect changes to your
source directory and re-invoke `fx test` following each file
modification. Test rebuilding works as follows (with overrides listed inline).

- **All selected tests are rebuilt by calling `fx build <targets>`
  for each `fx test` invocation.**
  - Use `--[no-]build` to toggle this behavior.
- **If selected tests are in a specified package for your build
  (specified using `fx set --with-test`), the `updates` package
  is built and an OTA will be performed.**
  - Use `--[no-]updateifinbase` to toggle this behavior.
  - Warning: OTA will fail when targeting an emulator.

### Test selection options

The following options affect which tests are selected by `fx test` and how
selections are applied.

**`--host`** and **`--device`** select only host or device tests
respectively. This is a global setting and they cannot be combined.

**`--[no-]e2e`** controls whether to run end-to-end (E2E) tests.
E2E tests are not run by default because they have the potential
to put the device in an invalid state. **`--only-e2e`** implies
`--e2e`, and ensures that only E2E tests are selected.

**`--package`** (`-p`) and **`--component`** (`-c`) select within package or
component names respectively. Names preceded by neither select any test field.
Multiple selections may be changed by **`--and`** (`-a`). For example:

```posix-terminal
fx test --package foo -a --component bar //src/other --and --package my-tests
```

The above command line contains two selection clauses:

1. Package "foo" AND component "bar" (e.g. fuchsia-pkg://fuchsia.com/foo#meta/bar.cm).
1. Package "my-tests" AND //src/other.

Tests matching either of the above clauses are selected.

**`--[no-]allow-empty-selection`** toggles whether individual selection clauses
that match no tests are permitted (default `true`). Passing
`--no-allow-empty-selection` fails immediately if any specified clause matches
nothing, which is useful in scripts to ensure every query selects tests. If no
tests are selected in total, `fx test` always fails.

Test selections are fuzzy-matched using a Damerau-Levenshtein
distance of 3 by default (e.g. "my_tset" will match "my-test").
**`--fuzzy <N>`** can be used to override this value to `N`, where
0 means not to do fuzzy matching.

Suggestions are shown by default if no test matches a selection
clause. The number of suggestions (default 6) can be overridden using
**`--suggestions-count N`**, and suggestions can be disabled or enabled using
**`--[no-]show-suggestions`**.

### Execution options

Tests are executed in a specific way that maximizes throughput and
stability, but each element of this default may be overridden. Tests
are executed as follows (with overrides listed inline):

- **Each selected test is executed in the order they appear within `tests.json`**
  - Use `--random` to randomize this execution order.
- **All selected tests are run, starting at the beginning of the ordered list above.**
  - Use `--offset N` to skip `N` tests at the beginning of the list. Default is 0.
  - Use `--limit N` to run at most `N` tests from the offset. Default is no limit.
- **At most 4 tests may run in parallel, such that at most one of
those tests is "non-hermetic" (as determined by `test-list.json`).**
  - Use `--parallel N` to change this default. `--parallel 1` means to execute
  each test serially.
- **Tests run until they terminate themselves.**
  - Use `--timeout N` to wait at most `N` seconds per test.
- **Each test runs one time.**
  - Use `--count N` to run each test `N` times.
- **If any execution of a test is failed, the rest of the repeats will be skipped.**
  - Use `--no-fail-by-group` to continue running up to the limit specified
    by `--count`.
- **All test cases are run from each test.**
  - Use `--test-filter` to run only specifically named test cases.
- **Failed tests are recorded and execution continues with the next selected test.**
  - Use `--fail` (`-f`) to terminate all tests following the first failure.
- **Tests that specify a maximum log level in `tests.json` will fail
if logs at a higher severity are seen.**
  - Use `--[no-]restrict-logs` to toggle this behavior.
- **Tests components themselves choose the minimum log severity to emit.**
  - Use `--min-severity-logs` to override this minimum for all test components.
- **Test components are run using the Merkle root hash from build
artifacts, which ensures that the latest version built was successfully
pushed to the target and is being run.**
  - Use `--[no-]use-package-hash` to toggle this behavior.
- **Test cases that are disabled are not run.**
  - Use `--also-run-disabled-tests` to run disabled test cases anyway.
- **Test output logs contain only the last segment of the component
moniker, so they are easier to visually inspect.**
  - Use `--[no-]show-full-moniker-in-logs` to toggle this behavior.
- **Failing tests terminate following failure without waiting**
  - Use `--break-on-failure` to catch failing tests with [zxdb][zxdb-docs].
  - Use `--breakpoint=<location>` to install breakpoints at specific
    [locations][#basic-test-debugging].

  Note that using the `--breakpoint` option will significantly slow down your
  tests. It is highly recommended to only use this option in conjunction with
  `--test-filter`. `--break-on-failure` may be used with many tests with minimal
  impact to performance.
- **Debug Adapter Protocol (DAP) server can be enabled for zxdb**
  - Use `--enable-debug-adapter` (in conjunction with `--break-on-failure` or
    `--breakpoint`) to start the `zxdb` DAP server. Use this to interact with
    `zxdb` from [`zxdb-cli`][zxdb-cli-source]  Incompatible with
    `--use-existing-debugger`.
  - Use `--debug-adapter-port=<port>` to specify the port for the DAP server
    (an open port is randomly assigned if not specified).
- **Command line arguments to the test are completely controlled by test runners**
  - Append `--` to your arguments to pass remaining arguments verbatim to the
  test. For example: `fx test foo -- --argument_for_test`
  will pass `--argument_for_test` to the test itself.
- **Host tests will inherit a limited set of environment variables
from the user's environment automatically**
  - Use `--env` (`-e`) to add new `KEY=VALUE` environment variables
  to tests. This flag may be specified multiple times.
- **If no package server is running, a temporary server will be started
for the duration of `fx test`'s execution**
  - Use `--no-allow-temporary-package-server` to disable this behavior.
  - If an existing package server is found, a temporary one will not be started.

### Output options

`fx test` is intended for developer use cases and includes a simple terminal UI
that displays the status of tests as they are executing. The default output
behavior is as follows (with overrides listed inline):

- **A status display is shown at the bottom of the terminal, and it
is automatically updated to show what operations are currently
executing.**
  - Use `--[no-]status` to toggle status display.
  - Use `--status-lines N` to change the number of status output lines.
  - Use `--status-delay N` to change the refresh rate (default is
  0.033 or approximately 30hz). If your terminal is slow you may
  want to change this to 0.5 or 1.
- **Output is styled with ANSI terminal colors.**
  - Use `--[no-]style` to toggle this behavior.
  - Use `--simple` as shorthand for `--no-style --no-status`.
- **Test outputs are only shown for tests that fail.**
  - Use `--output` (`-o`) to show all test output (combine with
  `--parallel 1` to prevent interleaving).
  - Use `--no-output` to hide output explicitly, such as to
  override `--output` set in config.
  - Use `--slow N` (`-s N`) to show output only for test suites
  that take longer than `N` seconds to execute.
- **Logs are written to a timestamped `.json.gz` file under the build
directory specified by `fx status`.**
  - Use `--[no-]log` to toggle logging entirely.
  - Use `--logpath` to change the output path of the log.
- **Test artifacts are not streamed off of the device.**
  - Use `--artifact-output-directory` (`--outdir`) to specify a directory where
  artifacts may be streamed in the `ffx test` output format.
- **Debug printing is suppressed.**
  - Use `--verbose` (`-v`) to print debug information to the console.
  This data is *extremely* verbose, and is only useful to debug `fx
  test` itself.

## Log Format

`fx test` is designed to support external tooling by representing every
user-visible output as an "event" which is logged to a file during execution.

Log files are compressed using gzip. Each line of the decompressed
file is a single JSON object representing one event. The event
schema is currently defined in [this][fxtest-python-event] Python
file.

When the format is stabilized, it will be possible to build interactive
viewers and converters to other formats (such as [Build Event
Protocol][build-event-protocol]{:.external}).


## Common issues

### fx test does not work with emacs

The emacs compilation window does not emulate an xterm-compatible terminal,
resulting in an error like below:

```bash {:.devsite-disable-click-to-copy}
in _make_progress_bar raise ValueError("Width must be at least 3")
```

To solve this problem, run `fx test` with the `--no-status` option to disable
the status bar.

### Escape sequences appear in fx test output

Your terminal may not support ANSI color codes, which `fx test` fails to detect.

Pass the `--no-style` option to `fx test` to disable color output or the
`--no-status` option to disable the updating status bar. Passing the
`--simple` option to `fx test` is equivalent to
`--no-style --no-status`.

### I don't know where my log file is

You can set the location of the log by passing `--logpath` to `fx test`, though
this is recommended only for non-interactive use.

By default, your logs are stored in your Fuchsia output directory as timestamped
files. Print the path to the previous logs using `fx test -pr path`.

### Printing the log file dumps garbage into my terminal

`fx test` logs are gzipped by default. Use the following command to pretty
print the most recent log to your terminal:

```bash
cat `fx test -pr path` | gunzip | jq -C | less -R
```

This command does the following:

- Find the most recent log path (`fx test -pr path`).
- Pipe the log to `gunzip` to decompress the log.
- Pipe the decompressed log to `jq` to pretty-print it with color output (`-C`).
- Pipe the color output to `less` configured to display color (`-R`).

For convenience, you can add an alias for this command in your `.bashrc` file:

```bash
alias testlog='cat `fx test -pr path` | gunzip | jq -C | less -R'
```

<!-- Reference links -->

[fuchsia-source-checkout]: /docs/get-started/get_fuchsia_source.md
[build-event-protocol]: https://bazel.build/remote/bep
[fxtest-python-event]: https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/scripts/fxtest/python/event.py
[fxtest-source]: https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/scripts/fxtest/python
[zxdb-cli-source]: https://cs.opensource.google/fuchsia/fuchsia/+/main:scripts/debug/zxdb_cli/
[trf-docs]: /docs/development/testing/components/test_runner_framework.md
[zxdb-docs]: /docs/development/debugger/commands.md
[zxdb-testing-docs]: /docs/development/debugger/tests.md
