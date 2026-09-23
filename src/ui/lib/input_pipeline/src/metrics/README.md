# Input Pipeline Lib: Metrics Logging

This library is meant to provide helper functions for logging in Input
Pipeline code.

## Cobalt logging

Currently, all the helper functions in this library are utility functions for
initializing and using Cobalt logger for Input Pipeline specific metrics.

Any component that uses this library should have access to the
`MetricEventLoggerFactory` capability. For example:

```
// Component manifest for a component that uses this library.
{
    ...
    use: [
        ...
        {
            protocol: [ "fuchsia.metrics.MetricEventLoggerFactory" ],
        },
    ],
    ...
}
```
