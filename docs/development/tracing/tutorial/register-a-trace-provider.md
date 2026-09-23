# Register a trace provider

This page describes how to register a new trace provider in a Fuchsia system.

To participate in tracing, new trace providers must be registered with the
trace manager in the [Fuchsia tracing system][fuchsia-tracing-system].
(However, drivers don't have to register as a trace provider since the devhost
process does it through `libdriver.so`.)

To register a trace provider, the steps are:

1. [Register a component with the trace manager](#register-a-component--with-the-trace-manager).
2. [Set up capability routing](#set-up-capability-routing).

## Register a component with the trace manager {:#register-a-component--with-the-trace-manager .numbered}

To register a component as a trace provider, you can use the `libtrace-provider`
library to provide an asynchronous loop in your component's code.

See the examples below:

* {C++}

  Note: This example uses `fdio` to set up the FIDL channel with the trace manager.
  For more information, see [`fdio`][fdio].

  ```cpp
  #include <lib/async-loop/cpp/loop.h>
  #include <lib/async-loop/default.h>
  #include <lib/trace-provider/provider.h>
  // further includes

  int main(int argc, const char** argv) {
    // process argv

    async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
    trace::TraceProviderWithFdio trace_provider(
        loop.dispatcher(), "my_trace_provider");

    // further setup

    loop.Run();
    return 0;
  }
  ```

* {C }

  ```c
  #include <lib/async-loop/cpp/loop.h>
  #include <lib/async-loop/default.h>
  #include <lib/trace-provider/provider.h>

  int main(int argc, char** argv) {
    zx_status_t status;
    async_loop_t* loop;
    trace_provider_t* trace_provider;

    // Create a message loop.
    status = async_loop_create(&kAsyncLoopConfigNoAttachToCurrentThread, &loop);
    if (status != ZX_OK) exit(1);

    // Start a thread for the loop to run on.
    // Alternatively, use async_loop_run() to run on the current thread.
    status = async_loop_start_thread(loop, "loop", NULL);
    if (status != ZX_OK) exit(1);

    // Create the trace provider.
    async_dispatcher_t* dispatcher = async_loop_get_dispatcher(loop);
    trace_provider = trace_provider_create(dispatcher);
    if (!trace_provider) exit(1);

    // Do something...

    // Tear down.
    trace_provider_destroy(trace_provider);
    async_loop_shutdown(loop);
    return 0;
  }
  ```

* {Rust}

  ```rust
  fn main() {
      fuchsia_trace_provider::trace_provider_create_with_fdio();
      // ...
  }
  ```

## Set up capability routing {:#set-up-capability-routing .numbered}

For your component to request the appropriate tracing capabilities,
include the following field in the component manifest (`.cml`):

```json5
{
  include: [
    "trace/client.shard.cml",
  ],
  ...
}
```

This allows your component to communicate with the trace manager using the
`fuchsia.tracing.provider.Registry` protocol as well as forward the offer to
its direct children. If your component is a child of another component, you'll
need to include the `client.shard.cml` in each of the parents' cmls as well.

If your component uses a Chromium-based `fuchsia.web` service and you want
to be able to collect trace data from it, both the
`fuchsia.tracing.provider.Registry` and
`fuchsia.tracing.perfetto.ProducerConnector` capabilities need to be provided
to your `Context`. (To understand how capabilities are passed to the `Context`,
see [`fuchsia.web/CreateContextParams.service_directory`][fuchsia-web-protocol].)

Once you have registered your component as a trace provider, you can enable
tracing in your code. For more information, see the next
[Add tracing in your code][add-tracing-in-your-code] page.

<!-- Reference links -->

[fuchsia-tracing-system]: /docs/concepts/kernel/tracing-system.md
[fdio]: /docs/concepts/filesystems/life_of_an_open.md#fdio
[fuchsia-web-protocol]: https://fuchsia.dev/reference/fidl/fuchsia.web#CreateContextParams.service_directory
[add-tracing-in-your-code]: /docs/development/tracing/tutorial/add-tracing-in-code.md
