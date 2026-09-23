# Honeydew unit tests

[TOC]

## Execution

Below commands run all Honeydew unit tests:
```shell
# Configure to build Fuchsia on core.x64 along with Honeydew unit tests
fx set core.x64 --with-host //src/testing/end_to_end/honeydew:unit_tests

# Run the Honeydew unit test
fx test //src/testing/end_to_end/honeydew --host --output
```
