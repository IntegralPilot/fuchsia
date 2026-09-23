# Metric names expectation files

Fuchsia's infrastructure for reporting performance test results has a
set of checked-in expectations for the names of the metrics that each
performance test may produce.  This is also referred to as the metric
names allowlist.  This was added in
[https://fxbug.dev/42056406](https://fxbug.dev/42056406).

If the set of metrics produced by a performance test does not match
those listed in the test's expectations file, the test will fail when
run.  This means that if a test is changed such that the set of
metrics it produces also changes, its expectation file must be updated
to match.

For the tests in fuchsia.git, the expectation files are located in
[src/tests/end_to_end/perf/expected_metric_names/](/src/tests/end_to_end/perf/expected_metric_names/).

The expectation files are required for all new performance tests,
which includes all Python-based performance tests.

## Intended benefits

*   The expectation files are intended to help keep metric naming
    consistent.  They should make it easier to review the naming of
    metrics, and provide a way to enforce the reviews through OWNERS
    checks.

*   The expectation files should make it easier for test authors to
    see what other metric names are used, for comparison, because the
    list will be checked in to the codebase.

*   The expectation files should make it easier to see how the list of
    produced metrics has changed for a given CL.  They allow
    developers to be sure that a CL has not accidentally removed or
    renamed metrics.

*   The expectation files provide a check on the number of metrics
    that are added.  This is useful because there are costs to adding
    large numbers of metrics.

*   The expectation files should make it easier to ensure that the set
    of metrics produced by a test is deterministic.  Metrics may be
    produced dynamically, and there is a range of ways of producing
    them.  That means that without the expectation check, it is
    possible for a test's set of metrics to be non-deterministic.
    That is undesirable, because it leads to random gaps in the data
    produced by CI builds.

## How to update expectation files

One way to update the expectation file for a test is to run the test
locally with the environment variable
`FUCHSIA_EXPECTED_METRIC_NAMES_DEST_DIR` set, using an invocation like
this:

```sh
fx test --e2e -o perf_publish_example_test \
    --env FUCHSIA_EXPECTED_METRIC_NAMES_DEST_DIR=$(pwd)/src/tests/end_to_end/perf/expected_metric_names/
```

When this environment variable is set, the test will write an updated
expectation file into the directory specified by the environment
variable.  Note that this will not preserve optional metric names in
the expectation file.

When this environment variable is not set, the test will report an
error if the metrics produced by the test don't match those listed in
the expectation file, and it will print the differences.  You can
update the expectation file manually based on the diffs in the error
message if necessary.

## How to add an expectation file

If you are adding a new performance test, you can add an expectation
file for it with the following steps.

For tests using the `fuchsia_component_perf_test` GN template, the
steps involve manually creating a placeholder expectation file:

1.  Create an empty placeholder version of the expectation file in
    `src/tests/end_to_end/perf/expected_metric_names/`.

2.  Add the `expected_metric_names_filepath` argument in the GN
    declaration of the test (the `fuchsia_component_perf_test` GN
    template invocation).

3.  Run the test locally with `FUCHSIA_EXPECTED_METRIC_NAMES_DEST_DIR`
    set, as described above, to generate the file contents.

For tests using the `python_perf_test` GN template, the expectation
file name is specified both in GN and in the test code, so it is
possible to avoid the inconvenience of manually creating a placeholder
file by following these steps:

1.  Pass the name in the `expected_metric_names_filename` argument to
    `publish_fuchsiaperf()` in the Python test code.

2.  Set `expected_metric_names_filepaths = []` in the GN declaration
    of the test (the `python_perf_test` GN template invocation).

3.  Run the test locally with `FUCHSIA_EXPECTED_METRIC_NAMES_DEST_DIR`
    set, as described above, to generate the file.

4.  Set `expected_metric_names_filepaths` to the actual list of
    expectation files in the GN declaration of the test.

5.  Add the expectation file to your Git checkout with `git add
    src/tests/end_to_end/perf/expected_metric_names/`.

These steps for `python_perf_test` also work if the test has multiple
expectation files.

It is possible to add the file to the `BUILD.gn` file first, but the
GN build will fail if the file does not exist.

## Optional metrics

The expectation files may contain entries with the suffix
`[optional]`.  These metrics are allowed to be absent in the test's
output.  This allows metrics to be architecture-specific or
machine-specific.  These metrics should be commented to say why they
are optional.  (Comment lines start with "#".)

This also allows for metrics to be non-determistic in whether they are
produced or not.  However, having a non-deterministic metric will
usually be considered a bug that should be fixed.

## Disabling metrics

If a metric is disabled and will never be produced by the code in its
current state, it is better to remove it from the list or comment it
out than to mark it as optional.  Otherwise we are liable to forget to
remove the `[optional]` tag when the metric gets re-enabled, which
would mean that we won't notice if the metric later goes missing.

## Skipping summarization of metrics

By default, when using the Python `perf_publish` library, the fuchsiaperf files
that get published are the summarized versions.

This summarization does two things that are worth calling out:

*   We treat the first value in each fuchsiaperf entry as a warm-up run and drop
    it.
*   There may be multiple entries for the same test case (for
    multiple process runs), in which case we merge them.

Doing this as a postprocessing step has these benefits:

*   It avoids the need to implement this processing either upstream (in the C++
    perftest library or in similar libraries for other languages) or in
    downstream consumers.
*   The summary fuchsiaperf file is much smaller than the "raw data" fuchsiaperf
    files and hence more manageable.
*   The "raw data" fuchsiaperf files can still be made available for anyone who
    wishes to analyze the raw data.

In some cases though, this might be undesirable: to keep the initial iterations'
times instead of dropping them or to allow standard deviations to be reported
to Chromeperf and have them displayed in the graphs.

Therefore, summarization can be turned off by adding `[no-summarize-metrics]`
at the top of a metric names expectations file.
