# Fuchsiaperf format for performance test results

[TOC]

This document describes the JSON schema that Fuchsia benchmark results must
follow in order to be uploaded to the performance dashboard.

## JSON description

```json
[
    {
        "label":       string     // Name of the test case in the performance dashboard.
        "test_suite":  string     // Name of the test suite in the performance dashboard.
        "unit":        string     // One of the supported units (see below)
        "values":      [v1, v2..] // Numeric values collected in this test case
    },
    {
        ...
    }
]
```

Note that having multiple values in the `values` array is somewhat
vestigial. The consumers of fuchsiaperf files usually just use the mean of
the `values` array, and the array often just contains a single value.

### Supported units

In order to convert benchmark results to the format required by the performance
dashboard, `unit` must be one of the following strings, which describe the units
of the result's `values`.

* `nanoseconds`  or `ns`
* `milliseconds` or `ms`
* `bytes`
* `bytes/second`
* `frames/second`
* `percent`
* `count`
* `W` (watts)

### Metric direction

The `unit` string may optionally specify the metric direction (whether a smaller
or larger value is better) by appending it with an underscore.

* `_smallerIsBetter`
* `_biggerIsBetter`

For example: `ns_smallerIsBetter` or `bytes/second_biggerIsBetter`.

### Example

```json
[
    {
        "label": "Channel/WriteRead/64bytes",
        "test_suite": "fuchsia.microbenchmarks",
        "unit": "nanoseconds",
        "values": [105.45, 697.916667, 672.743056]
    },
    {
        "label": "Channel/WriteRead/1024bytes",
        "test_suite": "fuchsia.microbenchmarks",
        "unit": "nanoseconds",
        "values": [102.23, 1004.340278, 906.250000]
    }
]
```

## See also

*   [Fuchsiaperf producers](fuchsiaperf_producers.md): Libraries for
    producing `fuchsiaperf` files.
*   [Fuchsiaperf consumers](fuchsiaperf_consumers.md): Systems that
    consume `fuchsiaperf` files.
