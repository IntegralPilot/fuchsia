
# Catapult performance results converter

This directory contains the `catapult_converter` command line tool which
takes perf test results in [the Fuchsiaperf format] and converts them to
the [Catapult Dashboard](https://github.com/catapult-project/catapult)'s
[JSON "HistogramSet" format](
https://github.com/catapult-project/catapult/blob/HEAD/docs/histogram-set-json-format.md).

## Parameters

The Catapult dashboard requires the following parameters (called
"diagnostics") to be present in each HistogramSet that is uploaded to
the dashboard:

* pointId: This parameter is taken from the
  `--execution-timestamp-ms` argument.  The dashboard uses this value
  to order results from different builds in a graph.

* benchmarks: This parameter is taken from the `test_suite` field in
  the JSON input file.  This usually has the prefix "fuchsia.",
  e.g. "fuchsia.microbenchmarks".

* masters: The term "master" is an outdated term from when Buildbot
  was used by Chrome infrastructure.  The convention now is to use the
  name of the bucket containing the builder for this parameter.
  Despite the name of the field, this field contains a single string.

  Examples: `fuchsia.ci`, `fuchsia.global.ci`

  These names appear as headings in the "Bot" dropdown menu on
  https://chromeperf.appspot.com/report.

* bots: The term "bot" is an outdated term from when Buildbot was used
  by Chrome infrastructure.  The convention now is to use the name of
  the builder for this parameter.  Despite the name of the field, this
  field contains a single string.

  Examples: `peridot-x64-perf-dawson_canyon` (old name, which is to be
  dropped), `fuchsia-x64-nuc` (replacement name)

  These names appear as selectable entries in the "Bot" dropdown menu
  on https://chromeperf.appspot.com/report.

* logUrls: This parameter is taken from the `--log-url` argument.

  This should contain a link to the LUCI build log page (or a page on
  any similar continuous integration system) for the build that
  produced the perf test results.  The Catapult dashboard will show
  this link if you select a point on a performance results graph.

  Note: Although the converter requires the `--log-url` argument, the
  Catapult dashboard does not require the logUrls field in the
  HistogramSet data.

* fuchsiaIntegrationInternalRevisions/fuchsiaIntegrationPublicRevisions:
  Unlike Catapult, which uses the above pointId, Skia Perf is provided with
  Git commit hashes. To be compatible, use `--integration-internal-git-commit`
  and optionally `--integration-public-git-commit` or `--smart-integration-git-commit`
  to provide the commit hash the results were recorded from.

  Example: `7106610114a0e86f6c94be3724fb4d4c30141e40`

  As the results are intended to be uploaded from infra,
  `--integration-internal-git-commit` should refer to the private
  integration.git commit hash while `--integration-public-git-commit` should
  refer to the public integration.git commit hash if there is one.
  Likewise, results uploaded from smart-integration can pass `--smart-integration-git-commit`.


This is an optional parameter that the Catapult dashboard accepts:

* a_productVersions: The system version, like '0.20200123.2.1'.

For more information on Catapult's format, see [How to Write
Metrics](https://github.com/catapult-project/catapult/blob/HEAD/docs/how-to-write-metrics.md).

[the Fuchsiaperf format]: /docs/development/performance/fuchsiaperf_format.md
