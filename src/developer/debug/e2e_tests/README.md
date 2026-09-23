# ZXDB E2E Tests

This directory contains the test cases and supporting infrastructure for ZXDB's end-to-end tests.

The test cases are dynamically registered from the `.script` files found in the `scripts` directory,
and executed with the script runneer in `script_test.cc`. The script runner feeds the commands from
the script to the zxdb console, and matches the output with the expectations in the script file to
determine success or failure.

## Prerequisites

The tests rely on environment variables to determine several test parameters that are exposed as
C macros to the test framework. The following macros are currently set by the `BUILD.gn` file in
this directory:

  * `ZXDB_E2E_TESTS_BUILD_TYPE`: This allows the script runner to determine at runtime which build
    configuration is being used, and allows scripts to skip themselves for particular builds. This
    variable is optional, but expect test failures when using an unsupported build type.
  * `ZXDB_E2E_TESTS_SYMBOL_DIR`:  **Required** for all tests to find symbol data for the inferior
    programs. The symbols are hard linked to this directory from `root_out_dir` using the
    `symbols_dir_test_data` template which is executed at build time.
  * `ZXDB_E2E_TESTS_SCRIPTS_DIR`: **Required** to locate the script files under `root_out_dir`.
  * `ZXDB_E2E_TESTS_FFX_TEST_DATA`: **Required** when `ffx debug` has been compiled as an FFX
    subtool. If `ffx debug` has been compiled directly into FFX, then this is unused.

### FFX Isolate

The test framework does additional work before registering the tests with Gtest. Zxdb has a
dependency on FFX to bridge the FIDL <-> Unix socket, which is currently managed in
`ffx_debug_agent_bridge.cc`. This class manages an [FFX Isolate][ffx-isolate] which connects to the
target and provides a Unix socket for zxdb to connect to in order to initiate the connection with
`debug_agent`. Note that this is the inverse of how users typically interact with zxdb, where the
`ffx debug` plugin creates the pipe to the target using FFX libraries and spawns and manages a child
zxdb process.

The FFX Isolate depends on severeal environment variables to be set in order to properly and
completely connect to a target. These should be considered requirements for running these tests
except when noted otherwise.

  * `FUCHSIA_DEVICE_ADDR`: The IPv4 or IPv6 address of the target. This will be set by `fx` when
    running the tests locally, and will be present in the environment in infra. Alternatively, if
    you execute the tests manually (i.e. `$(fx get-build-dir)/host_x64/zxdb_e2e_tests`), you will
    need to provide this environment variable manually. You can get the values from
    `ffx target list`.
  * `FUCHSIA_SSH_PORT`: The port on the target listening for ssh connections. This will be set by
    `fx` when running test locally and will be present in the environment in infra. Similarly to
    `FUCSHIA_DEVICE_ADDR` this will need to be set manually if running the test executable directly.
  * `FUCHSIA_SSH_KEY`: The ssh key that is authorized to connect via ssh to the target. This is not
    required to run the tests locally if you have your Fuchsia ssh keys placed in the default
    location `$HOME/.ssh/fuchsia_ed25519`. This will be set in the environment in infra.
  * `FUCHSIA_TEST_OUTDIR`: Not required to run tests locally, but is important that it is used in
    the infra environment so the ffx logs are discoverable.

After the environment has been read and the configuration is set, the FFX isolate is initialized in
a temp directory and the target is added. From there, `ffx debug connect --agent-only` is spawned as
a child process until all test cases have completed. Once the tests are finished, the isolate
directory is removed and the isolate daemon is stopped (as a result of removing the isolate
directory).

### Subdirectories

  * `inferiors`: test programs that are compiled to a component and added to the Fuchsia Package
    `zxdb_e2e_inferiors`
  * `scripts`: scripts run by the script runner (see `script_test.cc`) these simulate a user
    interacting with the zxdb console

## How to disable a specific test

Specific tests may be individually disabled by prepending the file name with `DISABLED_`. For
example, to disable the `inline_frames.script` test, rename the file to
`DISABLED_inline_frames.script` with `git mv`. This is often preferable to disabling the entire test
suite to maintain maximal coverage while individual failures are resolved.


## Pitfalls

This section lists common pitfalls when writing an e2e test using the `.script` files
For general scripting rules, see [scripting.md][scripting.md]

### Don't use line number in your command
Tip: Try breaking on a function instead.

The following script works on a local machine, but fails in a CQ run:

```zxdb
[zxdb] until 9
▶  9
Source file ./../../src/developer/debug/e2e_tests/inferiors/async_rust_multithreaded.rs not found. You might want to adjust the source file remap setting. See "get source-map".
```

This is because the source file provider relies on the source file to display line-related information.

[ffx-isolate]: /docs/development/tools/ffx/development/integration_testing.md
[scripting.md]:/docs/development/debugger/scripting.md
