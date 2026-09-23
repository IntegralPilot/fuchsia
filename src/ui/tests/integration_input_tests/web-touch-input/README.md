# Web Touch Input Test

## Build the test

```shell
$ fx set <product>.<arch> --with //src/ui/tests/integration_input_tests/web-touch-input:tests
```

## Run the test

To run the fully-automated test, use this fx invocation:

```shell
$ fx test web-touch-input-test
```

To see extra logs:

```shell
$ fx test --min-severity-logs=DEBUG web-touch-input-test -- --verbose=2
```
