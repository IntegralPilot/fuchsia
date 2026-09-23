# inspect-system-tests

This component is a test suite that measures the actual time it
takes to query Inspect data from each component on the system.

The component implements the `fuchsia.test.Suite` protocol and is
therefore its own test runner.

When executed, this test first queries the Archivist for all
components running on the system, and it reports back each component
moniker it finds as a separate test case. When instructed to run a
test case, it first confirms that the referenced component exists
and then repeatedly queries that component's Inspect data.

After running, this test reports statistics about the returned data
in the test stdout and also includes
[fuchsiaperf-formatted](https://fuchsia.dev/fuchsia-src/development/performance/fuchsiaperf_format)
results as a custom artifact for further processing.

To obtain results, run:

```
$ ffx test run fuchsia-pkg://fuchsia.com/inspect-system-tests#meta/inspect-system.cm --output-directory <dir>
```

Where `<dir>` is a directory in which results should be published.
See
[here](https://fuchsia.dev/fuchsia-src/reference/platform-spec/testing/test-output-format)
for the output directory format.

## Building

To add this component to your build, append
`--with src/diagnostics/tests/inspect-system:tests`
to the `fx set` invocation.

## Testing

Run the system-wide test using the `inspect-system-tests` package:

```
$ fx test inspect-system-tests
```

Run unit tests for the metrics processing and output formatting
code using the `inspect-system-unittests` package:

```
$ fx test inspect-system-unittests
```
