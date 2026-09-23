# Diagnostics

Fuchsia has several observability systems that help you perform diagnostics
on a component, such as:

* [Logs](#logs)
* [Inspect](#inspect)
* [Archivist](#archivist)

## Logs {#logs}

Logs are time-ordered streams of diagnostic data emitted by Fuchsia programs.
They most commonly take the form of low frequency human-oriented text strings
describing changes to state in a subsystem.

### Contents {#log_contents}

A log contains several pieces of metadata that is mostly self-reported by each
component that generates a specific log. At a minimum, a log message contains
a timestamp and string contents.

Additionally, if a log message is written to the [`LogSink`] protocol using the
supported libraries it includes a severity, a PID, a TID, a count of prior
dropped logs, and a list of string tags. Not all of the metadata is required,
but the supported libraries ensure that every field has a value when writing
records to the socket obtained through LogSink.

[Archivist](#archivist) inserts additional metadata to the data that a
component writes to a socket. For more information about the fields that
Archivist provides for a Fuchsia device log, see
[Log records][diagnostics-schema-ref].

### Storage {#log_storage}

Currently all log stores are rotated on a first-in-first-out (FIFO) basis,
with newer messages overwriting older ones. Messages from any component can roll
out messages from any other component.

Logs in Fuchsia can be stored in the following types of memory:

* [Volatile](#volatile)
* [Persistent](#persistent)

#### Volatile {#volatile}

There are two in-memory stores for logs on a Fuchsia device:

*   The `klog` or [debuglog], which is a [128kb buffer in the kernel].
*   The `syslog` which is a configurable [buffer in the Archivist].

As you work with logs, it is important to
[understand how Fuchsia writes logs][recording], so that you can better locate
where a log may be stored as it explains which of these buffers is the intended
point of transit for your message.

#### Persistent {#persistent}

The [feedback data] component maintains a [persistent_disk_store] of messages
from the previous boot of the system.

You can see these messages when targeting your Fuchsia device with
[`ffx target snapshot`][ffx-target-snapshot-ref].

### Work with Logs

To begin working with logging, you may want to:

* [Understand how Fuchsia writes logs][recording]
* [Understand how to view Fuchsia logs][viewing]

## Inspect

Inspect provides components with the ability to expose structured, hierarchical
information about their state at runtime. Components host a mapped Virtual
Memory Object (VMO) using the Inspect Format to expose an Inspect Hierarchy
containing this internal state. Note: For more information about the Inspect
VMO file format, see [Inspect VMO file format][inspect-vmo-format-ref].

### Work with Inspect

To begin working with Inspect, you may want to:

* [Understand Fuchsia component inspection][inspect_dev_overview]
* [Understand Inspect discovery and hosting][inspect_discovery_hosting]

## Archivist

Archivist ingests data from Inspect and Fuchsia logs, making them available over
the [`fuchsia.diagnostics/ArchiveAccessor`][ArchiveAccessor-protocol] protocol.

The [Archivist][archivist] ingests lifecycle events from the Component
Framework through [`fuchsia.component.EventStream`][event_stream]:

- **Capability requested**: The Archivist receives `Capability requested` events
  for connections to `fuchsia.logger.LogSink` and `fuchsia.inspect.InspectSink`
  which allows it to attribute Inspect and logs.

### `CapabilityRequested` event

`CapabilityRequested` event is the mechanisms that provide logs and Inspect
with attribution metadata such as the moniker and component URL.

[Archivist-manifest] `expose`s `fuchsia.logger.LogSink` just like any other
protocol capability, but it also `use`s an event from the framework, binding it
to a protocol in its namespace:

To better understand how this works, take a look at the Archivist's CML file:

Note: The highlighted code shows how the various capabilities are requested.

```
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="src/diagnostics/archivist/meta/common.shard.cml" highlight="25-36" %}
```

This causes Component Manager to redirect incoming requests from the default
`fuchsia.io` namespace protocol to the
[`fuchsia.component.EventStream`][event-stream] protocol. Archivist receives
events on this protocol, retrieving attribution metadata from the [`EventHeader`]
sent by Component Manager along with the `LogSink` channel's handle. The moniker
included in the descriptor is constructed in the event header.

Configuring a `capability_requested` event for `LogSink` does not affect
capability routing itself, only the delivery of the channel to the Archivist as
an Event instead of as an `Open()`. This means that the CML for passing the
attributed `LogSink` remains the same for the rest of the component topology.
For more information, see [Life of a protocol open] and the [events documentation][cm-events].

#### Attributing LogSink connections {#attributing}

When a Fuchsia component wants to write logs, it must obtain a connection to a
[`fuchsia.logger.LogSink`][logsink-protocol] in its incoming directory,
typically provided by an instance of the Archivist.

Typical Fuchsia service connections are anonymous such that the server and
client have no identifying information about each other. The client only sees
the service in its namespace, e.g. `/svc/fuchsia.logger.LogSink`, and the
server sees an anonymous `Open()` request to its incoming namespace.

Attributed logging provides trustworthy attribution metadata to allow for
better monitoring, storage, querying, and presentation of logs. This attribution
metadata identifies the source of an incoming `LogSink` connection, which is
accomplish through the `CapabilityRequested` event.

## Related documentation

* [Event stream capability][event_capability]
* [Inspect discovery and hosting - Archivist section][archivist]

[inspect_dev_overview]: /docs/development/diagnostics/inspect/README.md
[event_stream]: https://fuchsia.dev/reference/fidl/fuchsia.component#EventStream
[inspect_discovery_hosting]: /docs/reference/diagnostics/inspect/tree.md
[archivist]: /docs/reference/diagnostics/inspect/tree.md#archivist
[event_capability]: /docs/concepts/components/v2/capabilities/event.md
[ArchiveAccessor-protocol]: /reference/fidl/fuchsia.diagnostics.md#archive_accessor
[Archivist-manifest]: /src/diagnostics/archivist/meta/archivist.cml
[cm-events]: /docs/concepts/components/v2/capabilities/event.md
[`EventHeader`]: https://fuchsia.dev/reference/fidl/fuchsia.component#EventHeader
[event-stream]: https://fuchsia.dev/reference/fidl/fuchsia.component#EventStream
[logsink-protocol]: /sdk/fidl/fuchsia.logger/logger.fidl
[Life of a protocol open]: /docs/concepts/components/v2/capabilities/life_of_a_protocol_open.md
[diagnostics-schema-ref]: /docs/reference/platform-spec/diagnostics/schema.md
[ffx-target-snapshot-ref]: https://fuchsia.dev/reference/tools/sdk/ffx#ffx_target_snapshot
[`LogSink`]: https://fuchsia.dev/reference/fidl/fuchsia.logger#LogSink
[debuglog]: /docs/reference/kernel_objects/debuglog.md
[128kb buffer in the kernel]: /zircon/kernel/lib/debuglog/debuglog.cc
[buffer in the archivist]: /src/diagnostics/archivist/src/logs/mod.rs
[recording]: /docs/development/diagnostics/logs/recording.md
[viewing]: /docs/development/diagnostics/logs/viewing.md
[feedback data]: /src/developer/forensics/feedback_data
[persistent_disk_store]: /src/developer/forensics/feedback_data/system_log_recorder/system_log_recorder.h
[attributing]: #attributing
[inspect-vmo-format-ref]: /docs/reference/platform-spec/diagnostics/inspect-vmo-format.md
