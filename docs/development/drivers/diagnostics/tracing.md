# Add tracing to a driver

This document describes how to add tracing to a driver in a Fuchsia system.

## Overview

For an overview of tracing, see [Fuchsia tracing system][fuchsia-tracing-system].
However, drivers don't need to specify a [trace provider][trace-provider]. The
driver host provides one for its drivers.

For a tutorial on tracing, see [Add tracing in your code][add-tracing-in-your-code].
And for the tracing API reference, see [Tracing: C and C++ macros][trace-c-cpp-macros].

## Add trace records {:#add-trace-records}

To add tracing a driver, you need to update the driver's source code
and `BUILD.gn` file.

### Source code update {:#source-code-update}

To add trace records in a driver component, update the source code to invoke
the `TRACE_*()` macros from `lib/trace/event.h`, for example:

```c++ {:.devsite-disable-click-to-copy}
#include <lib/trace/event.h>

void DoSomething(int a, std::string b) {
  TRACE_DURATION("example", "DoSomething", "a", a, "b", b);

  // Do something
}
```

The first two arguments to most macros are the trace category and the
event name. In this example, they are `example` and `DoSomething`
respectively.

Trace categories let you specify what types of data the tracing system
collects. If a category is not requested, no data is collected. Categories
don't need to be unique across the driver. One typically groups several
events under the same category.

An event name is included in the trace to describe the event. It is
typically unique for each event.

### BUILD.gn update {:#buildgn-update}

To pick up tracing support, add the following target to your driver's
`BUILD.gn`:

```gn
fuchsia_cc_driver("my_driver") {
  deps = [
    ...
    "//zircon/system/ulib/trace",
  ]
}
```

## Use tracing {:#use-tracing}

Use [`ffx trace`][ffx-trace] to record a trace and view the result with the
[Perfetto viewer][perfetto-viewer]{:.external}.

The example command below uses the `example` category described in the
[Source code update](#source-code-update) section above:

```none {:.devsite-disable-click-to-copy}
$ ffx trace start --categories "example,kernel:sched,kernel:meta"
```

The `kernel:sched,kernel:meta` categories need to be present if you
want to visualize the result. The visualizer wants to associate trace data
with threads and processes, and it needs the data provided by the kernel
through these categories.

For additional details, as well as instructions on tracing directly on a
Fuchsia device without `ffx`, see
[Record traces for performance analysis][ffx-trace].

## Further reading

See [Fuchsia tracing guides][fuchsia-tracing-guides] for more information.

<!-- Reference links -->

[fuchsia-tracing-system]: /docs/concepts/kernel/tracing-system.md
[trace-provider]: /docs/concepts/kernel/tracing-system.md#trace-providers
[add-tracing-in-your-code]: /docs/development/tracing/tutorial/add-tracing-in-code.md
[trace-c-cpp-macros]: /docs/reference/tracing/c_cpp_macros.md
[fx-workflows]: /docs/development/build/fx.md
[ffx-trace]: /docs/development/tools/ffx/workflows/record-traces.md
[perfetto-viewer]: https://ui.perfetto.dev/#!/
[fuchsia-tracing-guides]: /docs/development/tracing/README.md

