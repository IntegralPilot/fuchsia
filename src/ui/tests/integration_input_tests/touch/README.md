## Build the test

```shell
$ fx set <product>.<arch> --with //src/ui/tests/integration_input_tests/touch:tests
```

## Run the test

To run the fully-automated test, use this fx invocation:

```shell
$ fx test touch-input-test
```

To see extra logs:

```shell
$ fx test --min-severity-logs=DEBUG touch-input-test -- --verbose=2
```

### Add trace metrics to the test

This test suite uses the category `touch-input-test` to log trace events. Any new categories added
to a test will need to be included in the `ffx trace start` command below.

Trace event types can be found in
[`libtrace`](//zircon/system/ulib/trace/include/lib/trace/event.h).

### Record a trace of the test

Add the touch tests to the base package set:

```shell
$ fx set <product>.<arch> --with //src/ui/tests/integration_input_tests/touch:tests

```shell
$ ffx trace start --categories touch-input-test --background
$ fx test touch-input-test
$ ffx trace stop
```
## Play with clients

You can use [`tiles-session`](/src/ui/bin/tiles-session/README.md) to manually run and
interact with any of the child clients under test.

### Play with the C++ Flatland client

To play around with the C++ Flatland client used in the automated test, invoke the client like this:

```shell
$ ffx session add fuchsia-pkg://fuchsia.com/touch-flatland-client#meta/touch-flatland-client.cm
```

### Play with the web client

To play around with the web client used in the automated test, invoke the client like this:

```shell
$ ffx session add fuchsia-pkg://fuchsia.com/one-chromium#meta/one-chromium.cm
```
