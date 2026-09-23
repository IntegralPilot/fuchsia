# Audio Signal Processing

## Overview

The signal processing interface is available to be potentially used by audio composite drivers.
This interface `SignalProcessing` is a FIDL protocol used by the `Composite` protocol to provide
audio signal processing capabilities.

The `SignalProcessing` protocol is defined to control signal processing hardware and their
topologies. We define processing elements (PEs) as a logical unit of audio data processing provided
by an audio driver, and we define topologies as the arrangement of PEs in [pipelines][pipeline] and
controls associated with them.

The `SignalProcessing` protocol allows hardware vendors to implement drivers with stable
application binary interfaces (ABIs), and allow system integrators to configure drivers to perform
differently based on system or product requirements using these interfaces for run-time
configurations.

The `SignalProcessing` protocol composes the `Reader` signal processing protocol. Signal processing
methods that only retrieve information are part of the `Reader` protocol, the rest are part of the
`SignalProcessing` protocol itself. This separation allows clients of this interface to compose the
`Reader` signal processing protocol into their own protocol if they require providing a read only
subset of functionality to their own clients.

The `SignalProcessing` protocol and associated definitions are part of the
[fuchsia.hardware.audio.signalprocessing](/sdk/fidl/fuchsia.hardware.audio.signalprocessing)
FIDL library.

### Topologies

Each driver can have its own topology. Drivers can abstract from applications the topologies
exposed by other drivers as needed for a particular configuration or product. Note that it is
possible although not required to expose topologies to applications, in particular to `audio_core`.


Notes:

* Topologies are not meant to fully describe the audio pipeline state/format/configuration
in and out of every PE. The intent is to describe what can be changed/rearranged by the client
based on its knowledge, configuration (for instance from metadata) and specific business logic.
* Topologies include one or more processing elements of type `RingBuffer`, if (and only if) ring
buffers are supported by the driver.
* Topologies include one or more processing elements of type `DaiInterconnect`, if (and only if)
DAI interconnects are supported by the driver.
*Topologies include one or more processing elements of type `PackeetStream`, if (and only if)
packet streams are supported by the driver.
* Topologies must include at least one of these element types.

### Processing Elements

A PE (defined in the `fuchsia.hardware.audio.signalprocessing` FIDL library as `Element`) is
expected to be hardware-provided functionality managed by a particular driver (but it could be
emulated in software, as any other driver functionality). A pipeline is composed of one or more PEs
and a topology is composed of one or more pipelines.

We refer to the server as the driver that is providing the signal processing protocol.
We refer to the client as the user of the functionality, e.g. an application such as `audio_core`.

## Basic operation

The client is responsible for requesting and then configuring any signal processing capabilities.
Once the server provides its PEs by replying to a client's `GetElements`, the client may
issue `WatchElementState` calls (see [hanging get pattern][hanging-get]) to retrieve
PE state and `SetElementState` to dynamically control the PEs parameters as needed. For
instance, to retrieve the `gain` of a PE of `type` `GAIN`, the client issues
`WatchElementState` calls, one to retrieve the initial state (the driver will reply to the
first `WatchElementState` sent by the client), and subsequent ones to get notified of updates
to the `ElementState` that includes the `gain`. Similarly, to retrieve the state of a PE
of `type` `EQUALIZER`, which is composed of multiple bands in its `bands_state`, a client would
issue a `WatchElementState` that would retrieve the initial state (the driver will reply to the
first `WatchElementState` sent by the client) including for instance `frequency` fields for
each band.

Also after the server provides its PEs by replying to a client's `GetElements`, the client
may request available topologies with the `GetTopologies` method. If more than one topology is
returned by `GetTopologies`, then `SetTopology` can be used to pick the topology to use, and
`WatchTopology` can be used to observe updates to the active topology.

### GetElements

`GetElements` allows to optionally get a list of all PEs. For instance this method may
be called by a client on a driver abstracting a hardware codec. Once the list of PEs is known to
the client, the client may configure the PEs based on the parameters exposed by the PE types.

### SetElementState

`SetElementState` allows a client to control the state of a PE using an id returned by
`GetElements`. PEs of different types may have different state exposed to clients, the
`SetElementState` parameter `state` has a different type depending on the type of PE.

### WatchElementState

`WatchElementState` allows a client to monitor the state of a PE using an id returned by
`GetElements`. PEs of different types may have different state exposed to clients, the
`WatchElementState` parameter `state` has a different type depending on the type of PE.

The `state` of a PE is composed of values that may be changed directly by the client via a call to
`SetElementState`, or indirectly for instance by calling `SetElementState` on a
different PE, or independent of the client for instance due to a plug detect change.

### GetTopologies

`GetTopologies` allows to optionally get a list of topologies. For instance this method may be
called by a client on a driver abstracting a hardware codec. Once the list of topologies is known to
the client, the client may configure the server to use a particular topology.

### SetTopology

`SetTopology` allows a client to control which topology is used by the server. Only one
topology can be selected at any time.

### WatchTopology

`WatchTopology` allows a client to observe the currently active topology id via a hanging get,
and to be notified asynchronously when the topology changes.

## Processing elements types

The PEs returned by `GetElements` support a number of different types of signal processing
defined by the PE types and parameters. PE types define standard signal processing (e.g. `GAIN`,
`DELAY`, `EQUALIZER`, etc), vendor specific signal processing (`VENDOR_SPECIFIC` e.g. a type not
defined in the `SignalProcessing` protocol), and `CONNECTION_POINT`s used to construct
multi-pipeline topologies (allow for routing and mixing definitions, see
[Connection points](#connection-points) below).

Each individual PE may have one or more inputs and one or more output channels. For routing and
mixing, PEs may make the number of output channels different from the number of input channels.

Data in each channel (a.k.a. the signal that is processed) may be altered by the PE. For instance
if there is a single PE of type `AGL` in a pipeline that includes a `DAI_INTERCONNECT`
element with `DaiFormat` `number_of_channels` set to 2, then AGL (Automatic Gain
Limiting) can be started or stopped for these 2 channels by a client calling `SetElementState`
with `state` `started` set to true (or false if the AGL `Element`'s `can_stop` was true).

If optional fields in the different PE types are not included, then the state of the processing
element is not changed with respect to the particular field. For instance, if an
`EqualizerBandState` in a `SetElementState` call does not include an optional `frequency` then the
equalizer's band frequency state is not changed.

## Vendor specific data

`ElementState` `vendor_specific_data` is an optional parameter that can be specified for any
processing element. This allows processing elements to specify an opaque object to be either sent
to the driver as part of a `SetElementState` or received from a driver as part of a
`WatchElementState`.

In addition to opaque data for any type, a processing element of type `VENDOR_SPECIFIC` allows
drivers to specify a type that is not defined in the `SignalProcessing` protocol, for instance
something that is not standard yet or is not meant to be standardized and provided only by a
specific vendor. A processing element of type `VENDOR_SPECIFIC` specifies a `vendor_specific`
variant in `TypeSpecificElement` and `TypeSpecificElementState`. In addition, it may specify
opaque data to be sent to or received from the driver using the `ElementState`
`vendor_specific_data` parameter, just like any other processing element type.

## Topologies {#topologies}

The topologies returned by `GetTopologies` support different arrangements for the PEs returned by
`GetElements`. `GetTopologies` may advertise one or multiple topologies.

### One topology

If one topology is advertised, i.e. `GetTopologies` returns a vector with one element, then all PEs
are part of this explicit single pipeline. Ordering in this case is explicit. For instance, if
`GetElements` returns 2 PEs:

1. `Element`: id = 1, type = `AUTOMATIC_GAIN_LIMITER` (AGL)
1. `Element`: id = 2, type = `EQUALIZER` (EQ)

The one `Topology` element returned by `GetTopologies` will list an `id` and a
`processing_elements_edge_pairs` vector explicitly advertising the order in which signal processing
is performed, in this example:

1. `Topology`: id = 1, `processing_elements_edge_pairs` = vector with one element with
`processing_element_id_from` = 1 and `processing_element_id_to` = 2.

This advertises this one topology with one pipeline:

                    +-------+    +-------+
    Input signal -> |  AGL  | -> |  EQ   | -> Output signal
                    +-------+    +-------+

In this topology the beginning (where the input signal is input into the pipeline) and the end of
the pipeline (where the output signal is output from the pipeline) are implicit.

If only one topology is advertised, then the contents are informational only since the client can't
change the use of one and only topology.

### Multiple topologies {#multiple-topologies}

If multiple topologies are advertised, i.e. `GetTopologies` returns a vector with multiple
elements, then PEs may be used in multiple configurations, i.e. topologies. Each topology
explicitly lists a number of PEs and their ordering, i.e. ordering in this case is explicit.
The arrangement and ordering of PEs define a pipeline.

By listing only the specific arrangements and ordering of PEs supported, servers restrict what
combinations of pipelines are valid.

For instance, if `GetElements` returns 6 PEs:

1. `Element`: id = 1, type = `AUTOMATIC_GAIN_LIMITER` (AGL)
1. `Element`: id = 2, type = `EQUALIZER` (EQ)
1. `Element`: id = 3, type = `SAMPLE_RATE_CONVERSION` (SRC)
1. `Element`: id = 4, type = `GAIN`
1. `Element`: id = 5, type = `DYNAMICS` (DRC1)
1. `Element`: id = 6, type = `DYNAMICS` (DRC2) parameters different from
DRC1 parameters.

The `Topology` elements returned by `GetTopologies` will list an `id` and a
`processing_elements_edge_pairs` for each topology, in this example:

1. `Topology`: id = 1, `processing_elements_edge_pairs` =
   * `processing_element_id_from` = 3 and `processing_element_id_to` = 2.
   * `processing_element_id_from` = 2 and `processing_element_id_to` = 4.
   * `processing_element_id_from` = 4 and `processing_element_id_to` = 5.
   * `processing_element_id_from` = 5 and `processing_element_id_to` = 1.
1. `Topology`: id = 2, `processing_elements_edge_pairs` =
   * `processing_element_id_from` = 2 and `processing_element_id_to` = 4.
   * `processing_element_id_from` = 4 and `processing_element_id_to` = 6.

This advertises two topologies with one pipeline each:

                    +-------+    +-------+    +-------+    +-------+    +-------+
    Input signal -> |  SRC  | -> |  EQ   | -> | GAIN  | -> |  DRC1 | -> |  AGL  | -> Output signal
                    +-------+    +-------+    +-------+    +-------+    +-------+

                    +-------+    +-------+    +-------+
    Input signal -> |  EQ   | -> | GAIN  | -> |  DRC2 | -> Output signal
                    +-------+    +-------+    +-------+

## Connection points {#connection-points}

The PEs of type `CONNECTION_POINT` allow for:

1. Mixing multiple channels within a single pipeline.
1. Mixing multiple channels from different pipelines.
1. Repeating channels.
1. Expanding a single pipeline into multiple pipelines ones (scatter).

{% comment %}
// TODO(https://fxbug.dev/42143529): Add extra context for multi-pipeline construction.
{% endcomment %}

<!-- Reference links -->

[pipeline]: https://en.wikipedia.org/wiki/Pipeline_(computing)
[hanging-get]: /docs/development/api/fidl.md#hanging-get

