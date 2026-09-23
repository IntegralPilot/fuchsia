# Record and visualize a trace

This page describes how to record and visualize a trace on a Fuchsia
device with the [Fuchsia tracing system][fuchsia-trace-system].

## Prerequisites

Important: Tracing is only enabled for `core` and `eng` build types in Fuchsia
images. Make sure that the Fuchsia image you use is of `core` or `eng` build type.
In other words, if you're to build a Fuchsia image from the Fuchsia source checkout
or download a Fuchsia prebuilt image, make sure that the image's build type is
**not** `user` or `userdebug`.

Many existing Fuchsia components are already registered as trace providers, whose
trace data often provide a sufficient overview of the system. For this reason,
if you only need to record a general trace (for instance, to include details
in a bug report), you may proceed to the sections below. However, if you want
to collect additional, customized trace events from a specific component, you need
to complete the following tasks first:

* [Register your component as a trace provider][register-a-trace-provider]
* [Add tracing in your component's code][add-tracing-in-your-code]

## Record a trace {:#record-a-trace}

To record a trace on a Fuchsia device from your host machine,
run the following command:

```posix-terminal
ffx trace start --duration <SECONDS>
```

This command starts a trace with the default settings, capturing
a general overview of the target device.

The trace continues for the specified duration (or until the `ENTER` key
is pressed if a duration is not specified). When the trace is finished, the
trace data is automatically saved to the `trace.fxt` file in the
current directory (which can be changed by specifying the `--output` flag;
for example, `ffx trace start --output <FILE_PATH>`). To visualize the trace
results stored in this file, see the [Visualize a trace](#visualize-a-trace)
section below.

Note: For more details on the `ffx trace` commands and options, see
[Record traces for performance analysis][record-traces].

## Visualize a trace {:#visualize-a-trace}

[Fuchsia trace format][fuchsia-trace-format] (`.fxt`) is Fuchsia's
binary format that directly encodes the original trace data. To
visualize an `.fxt` trace file, you can use the
[Perfetto viewer][perfetto-viewer]{:.external}.

You can visualize trace results using either of the following methods:

* **Upload to GCS:** Pass the `--upload` flag when recording (for example,
  `ffx trace start --upload`). Upon trace completion, the trace is uploaded to
  Google Cloud Storage (GCS) and a direct link to the Perfetto Trace Viewer
  is displayed in the output.
* **Open a local file manually:**
  1. Visit the [Perfetto viewer][perfetto-viewer]{:.external}
     site on a web browser.
  2. Click **Open trace file** on the navigation bar.
  3. Select your `.fxt` file from the host machine.

This viewer also allows you to query the trace data using SQL. For practical
query recipes and automated triage tools, see
[Analyze traces with PerfettoSQL and fx perf-analyze][analyze-traces-with-sql].

## Categories

Categories allow you to control which events you want to see. For example:

- Trace with all the default categories as well as "cat":

```posix-terminal
  ffx trace start --categories '#default,cat'
```

- Trace with no defaults, only scheduler and thread/process data:

```posix-terminal
  ffx trace start --categories "kernel:sched,kernel:meta"
```

### Useful categories

#### kernel:sched + kernel:meta

High granularity overview of what is running on each CPU

![kernel:sched information](images/sched.png "A view of the scheduler data in
the Perfetto Viewer"){: width="600"}

#### kernel:ipc

Emit an event for each FIDL call and connect them with flows

![kernel:ipc information](images/ipc.png "A view of the ipc data in
the Perfetto Viewer"){: width="600"}

#### kernel:syscall

Emit an event for every syscall system wide

![gfx information](images/syscall.png "A view of syscall events in the
Perfetto Viewer"){: width="600"}

#### gfx

View frame timing breakdowns

![gfx information](images/gfx.png "A view of events emitted each frame in the
Perfetto Viewer"){: width="600"}

<!-- Reference links -->

[fuchsia-trace-system]: /docs/concepts/kernel/tracing-system.md
[register-a-trace-provider]: /docs/development/tracing/tutorial/register-a-trace-provider.md
[add-tracing-in-your-code]: /docs/development/tracing/tutorial/add-tracing-in-code.md
[record-traces]: /docs/development/tools/ffx/workflows/record-traces.md
[fuchsia-trace-format]: /docs/reference/tracing/trace-format.md
[perfetto-viewer]: https://ui.perfetto.dev
[analyze-traces-with-sql]: /docs/development/tracing/analyze-traces-with-sql.md
[perfetto-trace-processor]: https://perfetto.dev/docs/analysis/trace-processor
[chromium-trace-viewer]: https://github.com/catapult-project/catapult/tree/HEAD/tracing
[chrome]: https://google.com/chrome
[trace-event-profileing-tool]: https://www.chromium.org/developers/how-tos/trace-event-profiling-tool
[catapult-project]: https://github.com/catapult-project
[process-creation]: /docs/concepts/process/process_creation.md
[connection-cc]: https://cs.opensource.google/fuchsia/fuchsia/+/main:/src/storage/lib/vfs/cpp/connection/connection.cc
[blob-cc]: https://cs.opensource.google/fuchsia/fuchsia/+/main:/src/storage/blobfs/blob.cc
