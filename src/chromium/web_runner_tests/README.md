## `web_runner_tests`

Contains integration tests to ensure that Chromium is compatible with Fuchsia.

These actually test `web_engine` and are named this way for historical reasons.

## Build the test

```shell
$ fx set <product>.<arch> --with //src/chromium/web_runner_tests:tests
$ fx build
```

If `<product>` does not include WebEngine by default, you will need to add it:
```shell
$ fx set <product>.<arch> --with //src/chromium/web_runner_tests:tests --with //src/chromium:web_engine
$ fx build
```

## Run the test

To run all the tests, use this fx invocation:

```shell
$ fx test web_runner_tests
```

To run individual test suites, use these fx invocations:

```shell
fx test web_runner_tests -t -- --gtest_filter="WebRunnerIntegrationTest.*"
```

For more information about the individual tests, see their respective file
comments.