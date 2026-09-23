# Example C++ Trace Provider

This directory contains a simple C++ trace provider that demonstrates setting up a trace provider
and emitting the various types of trace events.

## Building and Running

To build and run this example add to your fx set command:

```
fx set <...> --with //src/performance/example_trace_provider
```

After `fx build`ing and running `fx serve`, start the component:

```
ffx component run /core/ffx-laboratory:example_trace_provider 'fuchsia-pkg://fuchsia.com/example_trace_provider#meta/example_trace_provider.cm'
```

## Tracing
Now we can take a trace of our component:

```
ffx trace start --duration 2 --categories "example"
```

The trace will be saved to `trace.fxt` in the current directory and you can upload it to
[ui.perfetto.dev](ui.perfetto.dev) to view the trace.
