# Tutorial on Fuchsia tracing

Tracing is a powerful observability tool that can assist in quickly getting a
high level overview of a running system to find and diagnose issues. You can
think of tracing like visualizable, queryable, and toggleable printf debugging.

![A Busy Fuchsia Trace](images/perfetto-overview.png "A busy Fuchsia trace
displayed in the Perfetto Viewer"){: width="600"}

By enabling various categories, you can visualize the following (and more):

- High granularity breakdown of which threads are scheduled on each core
- Customizable per component trace events and spans
- FIDL calls between processes and threads
- A record of every single syscall
- Network and file system activity
- High level understanding of how a component operates and communicates

Step 1 of this tutorial walks through through how to take a trace, what's
available to trace, and how to visualize it.

You can unlock deeper analysis by adding custom trace points to your component
as well. Steps 2 and 3 explain how to add new trace points to a component.

1. [Record and visualize a trace][record-and-visualize-a-trace].
2. [Register a trace provider][register-a-trace-provider].
3. [Add tracing in your code][add-tracing-in-your-code].

<!-- Reference links -->

[record-traces]: /docs/development/sdk/ffx/record-traces.md
[fuchsia-tracing-system]: /docs/concepts/kernel/tracing-system.md
[register-a-trace-provider]: /docs/development/tracing/tutorial/register-a-trace-provider.md
[add-tracing-in-your-code]: /docs/development/tracing/tutorial/add-tracing-in-code.md
[record-and-visualize-a-trace]: /docs/development/tracing/tutorial/record-and-visualize-a-trace.md
[intro-to-fuchsia-tracing]: /docs/development/tracing/tutorial/intro-to-fuchsia-tracing.md
