# Recording thermal counters in a trace

## Overview

Fuchsia tracing can capture temperature sensor readings when supported by the
hardware. These readings appear as counters in the trace data and are valuable
for two primary reasons:

* Ensuring physical comfort - Devices that come into contact with people or
  sensitive environments should not overheat.
* Indicating power consumption - Heat generation is a general byproduct of
  power consumption and can serve as an indirect measure of power usage.

## Enabling thermal counters

Temperature sampling is not enabled by default. To enable temperature sampling,
run the following command prior to capturing the trace:

```posix-terminal
ffx profile temperature logger start -s 500ms
```

Note: The [`ffx profile`][ffx-profile] command is used to enable sampling of
several different measurements in addition to temperature.

## Capture the trace

For general information on Fuchsia tracing and how to capture trace events
with [`ffx trace`][ffx-trace], see [Fuchsia tracing][fuchsia-tracing].

To include thermal counter data in your trace, specify the
`metrics_logger` category:

```posix-terminal
ffx trace start --categories "#default,metrics_logger"
```

{% dynamic if user.is_googler %}

## Device specific information

As thermal measurements are hardware dependent, Googlers can find additional
device specific information at [go/fuchsia-tracing-internal-docs](http://goto.google.com/fuchsia-tracing-internal-docs).
{% dynamic endif %}

[fuchsia-tracing]: /docs/development/tracing/README.md
[ffx-profile]: /reference/tools/sdk/ffx.md#ffx_profile
[ffx-trace]: /reference/tools/sdk/ffx.md#ffx_trace
