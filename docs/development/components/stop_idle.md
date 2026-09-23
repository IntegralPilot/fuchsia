# Stop a component when it is idle

Components usually are not doing work all the time. Most components are written
to be asynchronous, meaning they are often waiting for the next FIDL message to
arrive. Nonetheless, these components occupy memory. This is a guide for
adapting your component to stop voluntarily and free up resources when it is
idle.

## Overview

Here's what to expect:

- You'll make some changes to your component's code such that it can decide when
  to stop. Your component will persist its state and handles right before
  stopping. Persisting this data is called *escrowing*.

- Clients to your component will not be aware that your component stopped.
  Stopping your component this way does not break their FIDL connections to your
  component.

- Fuchsia provides libraries that let you monitor when FIDL connections and
  the outgoing directory connection become idle, and turn those connections back
  to handles when that happens.

- Component Framework provides APIs for your component to store handles and data
  and retrieve them upon the next execution, typically after a handle is
  readable or upon a new capability request. We'll go into detail how they work
  in the next sections.

- Fuchsia snapshots and Cobalt dashboards will contain useful lifecycle metrics.

## What components are good candidates?

We recommend looking into components with these characteristics:

- **Spiky traffic**. The component can start and process those traffic, then go
  back to stopped when it's done. Lots of components in the boot and update path
  are only needed during those times, but otherwise are sitting around wasting
  RAM e.g. `core/system-update/system-updater`.

- **Isn't too stateful**. You can persist state before the component stops. In
  the limit, we could write code to persist all important state. In practice, we
  make trade-offs between the memory savings and the complexity of persisting
  the necessary state.

- **High memory usage**. Look at memory usage of your component using `ffx
  profile memory`. For example, it shows the `console-launcher.cm` on a typical
  system using `732 KiB` of private memory. Private memory is memory only
  referenced by that component so we're guaranteed to free at least that amount
  of memory when stopping that component. See
  [Measuring memory usage][measuring-memory-usage].

  ```text
  Process name:         console-launcher.cm
  Process koid:         2222
  Private:              732 KiB
  PSS:                  1.26 MiB (Proportional Set Size)
  Total:                3.07 MiB (Private + Shared unscaled)
  ```

[`http-client.cm`][http-client.cm] is an example component that doesn't hold
state across HTTP loader connections and is only used for metrics and crashes
uploading. Hence we have adapted it to stop when idle once configured as such.

## Known limitations

- **Hanging-gets**: if your component is the server or client of a hanging-get
  FIDL method, it will be challenging to preserve that connection because the
  FIDL bindings don't have a way to save and restore information about
  in-progress calls. You may convert that FIDL method to an event and a one-way
  ack.

- **Directories**: if you component serves directory protocols, it will be
  challenging to preserve that connection because directories are usually served
  by VFS libraries. The VFS libraries currently don't expose a way to get back
  the underlying channels and associated state (such as the seek pointer).

All these can be supported with enough justification. You may get in touch with
the Component Framework team with your use case.

## Detecting idleness

The first step to stopping an idle component is to enhance that component's code
to know when it has become idle, which means:

- **FIDL connections are idle**: A component usually declares a number of FIDL
  protocol capabilities and clients will connect to those protocols when they
  need it. These connections shouldn't have pending messages that require the
  component's attention.

- **Outgoing directory is idle**: A component serves an outgoing directory that
  publishes its outgoing capabilities. There shouldn't be pending messages that
  represent capability requests to this component and there shouldn't be extra
  connections into the outgoing directory besides the one established by
  `component_manager`.

- **Other background business logic**: For example, if a component makes a
  network request in the background in response to a FIDL method, we may not
  consider that component to be idle unless that network request has finished.
  It's likely unsafe to for that component to stop in the middle of the request.

We have Rust libraries for detecting idleness in each case.
[https://fxbug.dev/332342122](https://fxbug.dev/332342122) tracks the same
feature for C++ components.

### Detect idle FIDL connections

You can use [`detect_stall::until_stalled`][until_stalled] to transform a Rust
FIDL request stream into one that unbinds the FIDL endpoint automatically if the
connection is idle over a specified timeout. You need to add your component to
the visibility list at `src/lib/detect-stall/BUILD.gn`. Refer to the API docs
and tests for details. Here's how `http-client.cm` uses it:

```rust
async fn loader_server(
    stream: net_http::LoaderRequestStream,
    idle_timeout: fasync::Duration,
) -> Result<(), anyhow::Error> {
    // Transforms `stream` into another stream yielding the same messages,
    // but may complete prematurely when idle.
    let (stream, unbind_if_stalled) = detect_stall::until_stalled(stream, idle_timeout);

    // Handle the `stream` as per normal.
    stream.for_each_concurrent(None, |message| {
        // Match on `message`...
    }).await?;

    // The `unbind_if_stalled` future will resolve if the stream was idle
    // for `idle_timeout` or if the stream finished. If the stream was idle,
    // it will resolve with the unbound server endpoint.
    //
    // If the connection did not close or receive new messages within the
    // timeout, send it over to component manager to wait for it on our behalf.
    if let Ok(Some(server_end)) = unbind_if_stalled.await {
        // Escrow the `server_end`...
    }
}
```

### Detect idle outgoing directory

You can use the
[`fuchsia_component::server::ServiceFs::until_stalled`][service_fs] method to
transform a `ServiceFs` into one that unbinds the outgoing directory server
endpoint automatically if there is no work in the filesystem. Refer to the API
docs and tests for details. Here's how `http-client.cm` uses it:

```rust
#[fuchsia::main]
pub async fn main() -> Result<(), anyhow::Error> {
    // Initialize a `ServiceFs` and add services as per normal.
    let mut fs = ServiceFs::new();
    let _: &mut ServiceFsDir<'_, _> = fs
        .take_and_serve_directory_handle()?
        .dir("svc")
        .add_fidl_service(HttpServices::Loader);

    // Chain `.until_stalled()` before calling `.for_each_concurrent()`.
    // This wraps each item in the `ServiceFs` stream into an enum of either
    // a capability request, or an `Item::Stalled` message containing the
    // outgoing directory server endpoint if the filesystem became idle.
    fs.until_stalled(idle_timeout)
        .for_each_concurrent(None, |item| async {
            match item {
                Item::Request(services, _active_guard) => {
                    let HttpServices::Loader(stream) = services;
                    loader_server(stream, idle_timeout).await;
                }
                Item::Stalled(outgoing_directory) => {
                    // Escrow the `outgoing_directory`...
                }
            }
        })
        .await;
}
```

### Wait for other background business logic

The `ServiceFs` won't produce more capability requests once it has yielded the
`Item::Stalled` message. That could be problematic if you have some background
work that prevent your component from stopping, but the `ServiceFs` has become
idle in the meantime and has prematurely unbound the outgoing directory
endpoint. To handle those situations, you can prevent the `ServiceFs` from
becoming idle. The `Item::Request` yielded by the `ServiceFs` contains an
[`ActiveGuard`][active-guard]. As long as an active guard is in scope, the
`ServiceFs` will not become idle and will keep yielding capability requests as
they come in.

Similarly, you may create an [`ExecutionScope`][execution-scope] to spawn all
background work related to the processing of a FIDL connection, and call
`ExecutionScope::wait()` to wait for them to complete. For example, the
`loader_server` function in `http-client.cm` will not return until that
background work is done, and this will in turn keep the `active_guard` in the
`Item::Request` in scope, preventing the `ServiceFs` from stopping.

## Escrow handles and state to the framework

Once a connection is idle and the library has given you an unbound server
endpoint, the next step is to escrow those handles, in other words, send them to
the component framework for safekeeping.

### Stateless protocols

> **Note:** Protocols need to be marked as `@discoverable` to support escrowing.
  This allows components to reconnect to channels after waking from idle.

Some FIDL connections don't carry state. Every request functions identically
whether they are sent on the same connection or over separate connections.
You may follow these steps for those connections:

- Declare the capability in the component manifest if not already. You may need
  to declare the capability if this protocol connection is derived from another
  connection, and is otherwise not normally served from the outgoing directory.

- Add `delivery: "on_readable"` when declaring the capability. You need to add
  your component to the `delivery_type` visibility list at
  `tools/cmc/build/restricted_features/BUILD.gn`. The framework
  will then monitor the readable signal on the server endpoint of
  new connection requests, and connect the server endpoint to the provider
  component when there is a message pending. Example:

  ```json5
  capabilities: [
      {
          protocol: "fuchsia.net.http.Loader",
          delivery: "on_readable",
      },
  ],
  ```

- Add a use declaration from `self` for the capability such that the program may
  connect to it from its incoming namespace. You may install the capability in
  the `/escrow` directory to distinguish it from other capabilities used by your
  component. Example:

  ```json5
  {
      protocol: "fuchsia.net.http.Loader",
      from: "self",
      path: "/escrow/fuchsia.net.http.Loader",
  },
  ```

- Connect to the capability from the incoming namespace, passing the unbound
  server endpoint from `detect_stalled::until_stalled`.

  ```rust
  if let Ok(Some(server_end)) = unbind_if_stalled.await {
      // This will open `/escrow/fuchsia.net.http.Loader` and pass the server
      // endpoint obtained from the idle FIDL connection.
      fuchsia_component::client::connect_channel_to_protocol_at::<net_http::LoaderMarker>(
          server_end.into(),
          "/escrow",
      )?;
  }
  ```

Altogether, this means the component framework will monitor the idle connection
to be readable again, and then send that capability back to your component when
that happens. If your component has stopped, this will start your component.

### Outgoing directory

We have to use a different API to escrow the main outgoing directory connection
(i.e. the one returned by `ServiceFs` in `Item::Stalled`) because that server
endpoint is the entry point from which all other connections are made to a
component. For ELF components, you can send the outgoing directory to the
framework via the `fuchsia.process.lifecycle/Lifecycle.OnEscrow` FIDL event:

- Add `lifecycle: { stop_event: "notify" }` to the your component `.cml`:

  ```json5
  program: {
      runner: "elf",
      binary: "bin/http_client",
      lifecycle: { stop_event: "notify" },
  },
  ```

- Take the lifecycle numbered handle, turn it into a FIDL request stream, and
  send the event using `send_on_escrow`:

  ```rust
        {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/components/lifecycle/rust/src/lifecycle.rs" region_tag="escrow_listening" adjust_indentation="auto" %}
  ```

  Once your component has sent the `OnEscrow` event, it will not be able to
  monitor more capability requests. Hence it should promptly exit after that.
  Upon the next execution, your component will get back in its startup info the
  same `outgoing_dir` handle that it sent away in its previous run.

  Refer to [`http-client`][http-client] for how all these are put together.

### Inspect data

When a component stops, its Inspect VMO is no longer served. However, it's
possible to *escrow* a frozen, copy-on-write version of the Inspect data with
the framework. This lets diagnostics tools still read the last state of
the component's Inspect tree even after it has stopped.

The `inspect_runtime::publish` function returns a
[`PublishedInspectController`][inspect-controller-rustdoc]. Before your
component stops, you can call the `escrow_frozen` method on this controller.
This provides the framework with a frozen copy of your component's Inspect
VMO.

Here is an example of how to use it:

```rust
// When initializing your component...
let inspector = finspect::Inspector::default();
let inspect_controller =
    inspect_runtime::publish(&inspector, inspect_runtime::PublishOptions::default());

// ...

// When your component is about to stop...
if let Some(controller) = inspect_controller {
    let frozen_inspect_token = controller
        .escrow_frozen(inspect_runtime::EscrowOptions::default())
        .await;
    // The token can be stored in the escrowed dictionary if needed,
    // though often just escrowing is sufficient.
}
```

### Stateful protocols, and other important state

The `fuchsia.process.lifecycle/Lifecycle.OnEscrow` event takes another argument,
an `escrowed_dictionary client_end:fuchsia.component.sandbox.Dictionary` which
is a reference to a `Dictionary` object. [Dictionaries][dictionaries] are
key-value maps that may hold data or capabilities.

- You may create a `DictionaryRef` by using `fuchsia.component.sandbox.CapabilityStore`
  from framework, and calling `DictionaryCreate` on the `Factory` protocol:

  ```json5
  use: [
      {
          protocol: "fuchsia.component.sandbox.CapabilityStore",
          from: "framework",
      }
  ]
  ```

  ```rust
        {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/components/lifecycle/rust/src/lifecycle.rs" region_tag="escrow_create_dictionary" adjust_indentation="auto" %}
  ```

- You may add some data (e.g. a vector of bytes) to the `Dictionary` by calling
  `Insert` on the `Dictionary` FIDL connection. Refer to the
  [`fuchsia.component.sandbox`][sandbox-fidl] FIDL library documentation for
  other methods:

  ```rust
        {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/components/lifecycle/rust/src/lifecycle.rs" region_tag="escrow_populate_dictionary" adjust_indentation="auto" %}
  ```

- Before exiting, send the `Dictionary` client endpoint in `send_on_escrow`:

  ```rust
        {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/components/lifecycle/rust/src/lifecycle.rs" region_tag="escrow_send_dictionary" adjust_indentation="auto" %}
  ```

- On next start, you may obtain this dictionary from the startup handles:

  ```rust
        {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/components/lifecycle/rust/src/lifecycle.rs" region_tag="escrow_receive_dictionary" adjust_indentation="auto" %}
  ```

The `Dictionary` object supports a variety of item data types. If your
component's state is less than `fuchsia.component.sandbox/MAX_DATA_LENGTH`,
you may consider storing the `fuchsia.component.sandbox/Data` item,
which can hold a byte vector.

## I want to wait for a channel to be readable

Prior to stopping, if you would like to arrange for the component framework to
wait until a channel to be readable, and then pass the channel back to your
component, you may use the same `delivery: "on_readable"` technique. This
generalizes to FIDL protocols that are not exposed by your component, such as
service members. It even supports channels that do not speak FIDL protocols. As
an example, suppose your component holds a Zircon exception channel, and needs
to tell the framework to wait for that channel to be readable and then start
your component, you may declare the following `.cml`:

```json5
capabilities: [
    {
        protocol: "exception_channel",
        delivery: "on_readable",
        path: "/escrow/exception_channel",
    },
],
use: [
    {
        protocol: "exception_channel",
        from: "self",
        path: "/escrow/exception_channel",
    }
]
```

Note that the `exception_channel` capability is not exposed. This capability is
used by the component itself. The component may open `/escrow/exception_channel`
from its incoming namespace with the channel to be waited on. When that channel
is readable, the framework will open `/escrow/exception_channel` in the outgoing
directory, starting the component if needed. In summary, you may declare
capabilities and use them from `self` to escrow a handle to `component_manager`.

Get in touch with the Component Framework team if you need other kinds of
triggers, such as waiting for custom signals or waiting for a timer.

## Testing

We recommend enhancing existing integration tests to also test that your
component can stop itself and start again without breaking FIDL connections.
If you already have an integration test that starts up your component and send
FIDL requests to it, you may use the component event matchers to verify that
your component stops when there are no messages. Refer to the
[`http-client` tests][http-client-test] for an example of how that's done.

## Landing and metrics

If there are specific products you would like to optimize this component for,
you may add structured configuration to your component that controls if/how long
the idle timeout is.

The component framework records how long your component started and stopped in
between executions and uploads those to Cobalt. You may view them in this
[dashboard] to fine-tune the idle timeout.

When a feedback snapshot is taken, such has when a bug is encountered in the
field, the timestamps of the initial and latest component executions will be
available at selector `<component_manager>:root/lifecycle/early` and
`<component_manager>:root/lifecycle/late` respectively. You may correlate those
events with other error logs to assist in investigating if an error is caused
by improper stopping of components.

<!-- xrefs -->

[measuring-memory-usage]: /docs/development/tools/ffx/workflows/explore-memory-usage.md
[http-client.cm]: https://cs.opensource.google/fuchsia/fuchsia/+/50b8825378e19078d84171ce21f9eb3d7e22d6db:src/connectivity/policy/http-client/meta/http_client.cml
[until_stalled]: https://fuchsia-docs.firebaseapp.com/rust/detect_stall/fn.until_stalled.html
[service_fs]: https://fuchsia-docs.firebaseapp.com/rust/fuchsia_component/server/struct.ServiceFs.html#method.until_stalled
[active-guard]: https://cs.opensource.google/fuchsia/fuchsia/+/6eb3df68a36e998290d272274445893970d96979:src/storage/lib/vfs/rust/src/execution_scope.rs;l=369
[execution-scope]: https://cs.opensource.google/fuchsia/fuchsia/+/6eb3df68a36e998290d272274445893970d96979:src/storage/lib/vfs/rust/src/execution_scope.rs;l=54
[http-client]: https://cs.opensource.google/fuchsia/fuchsia/+/50b8825378e19078d84171ce21f9eb3d7e22d6db:src/connectivity/policy/http-client/src/main.rs
[dictionaries]: /docs/contribute/governance/rfcs/0235_component_dictionaries.md
[sandbox-fidl]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.component.sandbox/sandbox.fidl
[http-client-test]: https://cs.opensource.google/fuchsia/fuchsia/+/565cbdce0f486511230a95fc8cc30106b25172fb:src/connectivity/policy/http-client/integration/src/lib.rs;l=565
[dashboard]: http://go/fuchsia-escrow-metrics
[inspect-controller-rustdoc]: https://fuchsia-docs.firebaseapp.com/rust/inspect_runtime/struct.PublishedInspectController.html
