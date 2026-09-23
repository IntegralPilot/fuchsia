# Audio Drivers Architecture

In Fuchsia there are many ways drivers can be architected as defined by the
number of drivers used, how they communicate and their responsibilities. Audio
drivers responsibilities are determined by the interface(s) exposed to driver
clients, clients could be other drivers or applications users of the drivers
facilities.

## Definitions

| Term                 | Definition                                           |
| -------------------- | ---------------------------------------------------- |
| Hardware Codec       | A real or virtual device that encodes/decodes a      |
:                      : signal from digital/analog to/from analog/digital    :
:                      : including all combinations, e.g. digital to digital. :
:                      : Example codecs include DAC-Amplifiers combos and ADC :
:                      : converters.                                          :
| Controller or engine | The hardware part of a system that manages the audio |
:                      : signals, for example an SOC's audio subsystem.       :
| DAI                  | Digital Audio Interface. Interface between audio     |
:                      : hardware for instance a TDM or PDM link between      :
:                      : controllers and codecs.                              :
| Ring buffer          | Abstracts management of the shared memory area (in   |
:                      : main memory) used for data transfer; this shared     :
:                      : memory area is provided by a VMO object.             :
| Packet stream        | Abstracts packet-based audio streaming (e.g. for     |
:                      : encoded audio formats such as AAC and SBC) via       :
:                      : discrete packet transfers.                           :

# Audio interfaces

The API to be used by applications/clients users of audio drivers is the
[Audio Composite Interface](composite.md). This API allows applications to
access audio hardware functionality exposed by drivers. It allows drivers to
expose the functionality of various types of hardware including hardware codecs
with one or more DAIs, controllers with
[Ring Buffers](/sdk/fidl/fuchsia.hardware.audio/ring_buffer.fidl),
[Packet Streams](/sdk/fidl/fuchsia.hardware.audio/packet_stream.fidl), and DAIs, along with
any combination of processing elements allowed by the
[Audio Signal Processing](signal-processing.md) APIs.

A common split in audio hardware is to have an audio engine that configures a
DAI communicating with an audio hardware codec. In this split we can have
one driver for the audio engine and one for the codec. Both drivers can expose
their relevant functionality using the [Audio Composite Interface](composite.md).
The codec driver would expose one or more DAI interconnect interfaces and the
audio engine driver would configure the audio engine hardware including the DAI
or DAIs that connect to the codec driver or drivers.

Clients of these drivers are expected to configure all drivers. An example
usage of this architecture for a system with two different codecs physically
connected to the engine, e.g. abstracting the audio subsystem of an SoC.

```
                           +-----------------+
                +----------+     Client      +----------+
                |          +-----------------+          |
                |                   |                   |
          Composite API       Composite API      Composite API
                |                   |                   |
       +-----------------+ +-----------------+ +-----------------+
       | audio subsystem | |     Codec 1     | |     Codec 2     |
       +-----------------+ +-----------------+ +-----------------+
```
# Deprecated interfaces

Deprecated interfaces include:

1. [StreamConfig](/sdk/fidl/fuchsia.hardware.audio/stream_config.fidl):
Used to capture or render audio by
[audio_core](/src/media/audio/audio_core/README.md) and
[audio-driver-ctl](/src/media/audio/tools/audio-driver-ctl). The former is the
version 1 of the core of the audio system (providing software mixing, routing,
etc.) and the latter is a utility used for testing and bringup of new platforms.

Drivers that previously would use the StreamConfig API can be implemented using
[Audio Composite](composite.md) with one Ring Buffer.

1. [Codec](/sdk/fidl/fuchsia.hardware.audio/codec.fidl): Used to abstract
hardware codecs with one DAI. Drivers that previously would use the Codec API
can be implemented using
[Audio Composite](composite.md) with one DAI and no Ring Buffers.

1. [DAI](/sdk/fidl/fuchsia.hardware.audio/dai.fidl): Used to abstract SoC
hardware with one Ring Buffer and one DAI. Drivers that previously would use the
DAI API can be implemented using
[Audio Composite](composite.md) with one DAI and one Ring Buffer.

# Virtual audio drivers

For automated testing and subsystem development, virtual audio drivers emulate
audio hardware in software:
* The modern **[`virtual-audio`](virtual-audio.md)** driver implements the
  `Composite` API.
* The legacy **[`virtual-audio-legacy`](virtual-audio.md#legacy-virtual-audio-driver-virtual-audio-legacy)**
  driver implements deprecated `StreamConfig`, `Dai`, and `Codec` APIs.



