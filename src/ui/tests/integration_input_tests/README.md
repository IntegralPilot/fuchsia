# Integration Input Tests

This collection of tests exercises the input dispatch paths in core components,
such as Scenic and Input Pipeline (as integrated as a part of Scene Manager).
They are intended to be fairly minimal, free of flakiness, and standalone -
the entire test is in one file.

## Building tests

To build and run the tests for core-based products (e.g. core, astro, or
sherlock), include the `integration_input_tests` test package in your build
args either directly:

<!-- TODO(https://fxbug.dev/42070261): Remove the web_engine lines when resolved. -->

```
fx set ... \
  --with //src/chromium:web_engine \
  --with //src/ui/tests/integration_input_tests
```

or transitively:

```
fx set ... \
  --with //src/chromium:web_engine \
  --with //bundles/tests
```

## Running tests

To run these, we can use `fx test` with the name of the corresponding
`fuchsia_test_package` name defined in the test's `BUILD`:

```shell
fx test factory-reset-test
fx test integration_input_tests
fx test touch-input-test
fx test text-input-test
fx test mouse-input-test
```
