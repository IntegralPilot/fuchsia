<!--

// LINT.IfChange

-->

# audio-codec-ctl

Audio hardware codec driver control. Used to control and get information from an audio hardware
codec driver via the FIDL codec interface.
See [`audio-driver-ctl`](/docs/reference/tools/hardware/audio-driver-ctl.md)
to play, record, and configure audio streams.

## Usage {#usage}

```none
  audio-codec-ctl [-d|--device <device>] f[ormats]

  audio-codec-ctl [-d|--device <device>] r[eset]

  audio-codec-ctl [-d|--device <device>] d[ai] <number_of_channels>
    <channels_to_use_bitmask> pdm|upcm|spcm|fpcm none|i2s|left-stereo|right-stereo|1tdm|2tdm|3tdm
    <frame_rate> <bits_per_slot> <bits_per_sample>

  audio-codec-ctl [-d|--device <device>] start

  audio-codec-ctl [-d|--device <device>] stop

  audio-codec-ctl [-d|--device <device>] p[lug_state]

  audio-codec-ctl [-d|--device <device>] e[elements]

  audio-codec-ctl [-d|--device <device>] t[opologies]

  audio-codec-ctl [-d|--device <device>] w[atch] <id>

  audio-codec-ctl [-d|--device <device>] set <id> [start|stop] [bypass] [gain <gain>]
    [vendor <hex> <hex> ...]

  audio-codec-ctl list

  audio-codec-ctl [-h|--help]
```

## Commands {#commands}

Audio hardware codec driver control on `<device>` (full path specified, for example, `/svc/fuchsia.hardware.audio.CodecConnectorService/default/codec_connector` or
only the service instance name specified, for example, `default`) or unspecified (picks the first device in
`/svc/fuchsia.hardware.audio.CodecConnectorService`). You can specify only one command per invocation.


### `list` command {#list}

```none
  audio-codec-ctl list
```

Lists all available codec devices.

### `formats` command {#formats}

```none
  audio-codec-ctl [-d|--device <device>] f[ormats]
```

Retrieves the DAI formats supported by the codec.

### `info` command {#info}

```none
  audio-codec-ctl [-d|--device <device>] i[nfo]
```

Retrieves textual information about the codec.

### `capabilities_plug_detect` command {#plugdetect}

```none
  audio-codec-ctl [-d|--device <device>] c[apabilities_plug_detect]
```

Retrieves Plug Detect Capabilities.

### `reset` command {#reset}

```none
  audio-codec-ctl [-d|--device <device>] r[eset]
```

Resets the codec.

### `dai` command {#dai}

```none
  audio-codec-ctl [-d|--device <device>] d[ai] <number_of_channels>
    <channels_to_use_bitmask> pdm|upcm|spcm|fpcm none|i2s|left-stereo|right-stereo|1tdm|2tdm|3tdm
    <frame_rate> <bits_per_slot> <bits_per_sample>
```

Sets the DAI format to use in the codec interface.

`<number_of_channels>`: Number of channels.

`<channels_to_use_bitmask>`: Sets which channels are active via a bitmask. The least significant
  bit corresponds to channel index 0.

`pdm`: Pulse Density Modulation samples.

`upcm`: Signed Linear Pulse Code Modulation samples at the host endianness.

`spcm`: Unsigned Linear Pulse Code Modulation samples at the host endianness.

`fpcm`: Floating point samples IEEE-754 encoded.

`none`: No frame format as in samples without a frame sync like PDM.

`i2s`: Format as specified in the I2S specification.

`left-stereo`: Left justified, 2 channels.

`right-stereo`: Right justified, 2 channels.

`1tdm`: Left justified, variable number of channels, data starts at frame sync changes from low to
  high clocked out at the rising edge of sclk. The frame sync must stay high for exactly 1
  clock cycle.

`2tdm`: Left justified, variable number of channels, data starts one clock cycle after the frame
  sync changes from low to high clocked out at the rising edge of sclk. The frame sync must
  stay high for exactly 1 clock cycle.

`3tdm`: Left justified, variable number of channels, data starts two clock cycles after the frame
  sync changes from low to high clocked out at the rising edge of sclk. The frame sync must
  stay high for exactly 1 clock cycle.

`<frame_rate>`: The frame rate for all samples.

`<bits_per_slot>`: The bits per slot for all channels.

`<bits_per_sample>`: The bits per sample for all samples.  Must be smaller than bits per channel
  for samples to fit.

### `start` command {#start}

```none
  audio-codec-ctl [-d|--device <device>] start
```

Start/Re-start the codec operation.

### `stop` command {#stop}

```none
  audio-codec-ctl [-d|--device <device>] stop
```

Stops the codec operation.

### `plug_state` command {#plug}

```none
  audio-codec-ctl [-d|--device <device>] p[lug_state]
```

Get the plug detect state.

### `elements` command {#elements}

```none
  audio-codec-ctl [-d|--device <device>] e[lements]
```

Returns a vector of supported processing elements.

### `topologies` command {#topologies}

```none
  audio-codec-ctl [-d|--device <device>] t[opologies]
```

Returns a vector of supported topologies.

### `watch` command {#watch}

```none
  audio-codec-ctl [-d|--device <device>] w[atch] <id>
```

Get a processing element state.

### `set` command {#set}

```none
  audio-codec-ctl [-d|--device <device>] set <id> [start|stop] [bypass] [gain <gain>]
    [vendor <hex> <hex> ...]
```

Controls a processing element.

`<id>`: Processing element id.

`start`: Process element started state.

`stop`: Process element stopped state.

`bypass`: Process element bypassed state.

`<gain>`: Current gain in GainType format reported in the supported processing elements vector.

`<hex>`: Vendor specific raw byte to feed to the processing element in hex format.

## Examples {#examples}

### List all available codec devices

```posix-terminal
audio-codec-ctl list
```

```none {:.devsite-disable-click-to-copy}
default
```

### Retrieve the DAI formats supported

```posix-terminal
audio-codec-ctl f
```

```none {:.devsite-disable-click-to-copy}
Executing on device: /svc/fuchsia.hardware.audio.CodecConnectorService/default/codec_connector
[ fuchsia_hardware_audio::DaiSupportedFormats{ number_of_channels = [ 2, 4, ], sample_formats = [ fuchsia_hardware_audio::DaiSampleFormat::kPcmSigned, ], frame_formats = [ fuchsia_hardware_audio::DaiFrameFormat::frame_format_standard(fuchsia_hardware_audio::DaiFrameFormatStandard::kI2S), fuchsia_hardware_audio::DaiFrameFormat::frame_format_standard(fuchsia_hardware_audio::DaiFrameFormatStandard::kTdm1), ], frame_rates = [ 48000, 96000, ], bits_per_slot = [ 16, 32, ], bits_per_sample = [ 16, 32, ], }, ]
```

### Retrieve textual information

```posix-terminal
audio-codec-ctl i
```

```none {:.devsite-disable-click-to-copy}
Executing on device: /svc/fuchsia.hardware.audio.CodecConnectorService/default/codec_connector
fuchsia_hardware_audio::CodecInfo{ unique_id = "", manufacturer = "Texas Instruments", product_name = "TAS5825m", }
```

### Retrieve plug detect capabilities

```posix-terminal
audio-codec-ctl c
```

```none {:.devsite-disable-click-to-copy}
Executing on device: /svc/fuchsia.hardware.audio.CodecConnectorService/default/codec_connector
fuchsia_hardware_audio::PlugDetectCapabilities::kHardwired
```

### Check if the codec is bridgeable

```posix-terminal
audio-codec-ctl b
```

```none {:.devsite-disable-click-to-copy}
Executing on device: /svc/fuchsia.hardware.audio.CodecConnectorService/default/codec_connector
Is bridgeable: false
```

### Reset the codec

```posix-terminal
audio-codec-ctl r
```

```none {:.devsite-disable-click-to-copy}
Executing on device: /svc/fuchsia.hardware.audio.CodecConnectorService/default/codec_connector
Reset done
```

### Set a codec's bridged mode

```posix-terminal
audio-codec-ctl -m true
```

```none {:.devsite-disable-click-to-copy}
Setting bridged mode to: true
Executing on device: /svc/fuchsia.hardware.audio.CodecConnectorService/default/codec_connector
```

### Set the DAI format to use in the codec interface

```posix-terminal
audio-codec-ctl d 2 1 s i 48000 16 32
```

```none {:.devsite-disable-click-to-copy}
Setting DAI format:
fuchsia_hardware_audio::DaiFormat{ number_of_channels = 2, channels_to_use_bitmask = 1, sample_format = fuchsia_hardware_audio::DaiSampleFormat::kPcmSigned, frame_format = fuchsia_hardware_audio::DaiFrameFormat::frame_format_standard(fuchsia_hardware_audio::DaiFrameFormatStandard::kI2S), frame_rate = 48000, bits_per_slot = 16, bits_per_sample = 32, }
Executing on device: /svc/fuchsia.hardware.audio.CodecConnectorService/default/codec_connector
```

### Start or restart the codec operation

```posix-terminal
audio-codec-ctl start
```

```none {:.devsite-disable-click-to-copy}
Executing on device: /svc/fuchsia.hardware.audio.CodecConnectorService/default/codec_connector
Start done
```

### Stop the codec operation

```posix-terminal
audio-codec-ctl stop
```

```none {:.devsite-disable-click-to-copy}
Executing on device: /svc/fuchsia.hardware.audio.CodecConnectorService/default/codec_connector
Stop done
```

### Get the plug detect state

```posix-terminal
audio-codec-ctl p
```

```none {:.devsite-disable-click-to-copy}
Executing on device: /svc/fuchsia.hardware.audio.CodecConnectorService/default/codec_connector
fuchsia_hardware_audio::PlugState{ plugged = true, plug_state_time = 1167863520, }
```

### Get the supported processing elements

```posix-terminal
audio-codec-ctl e
```

```none {:.devsite-disable-click-to-copy}
Executing on device: /svc/fuchsia.hardware.audio.CodecConnectorService/default/codec_connector
[ fuchsia_hardware_audio_signalprocessing::Element{ id = 1, type = fuchsia_hardware_audio_signalprocessing::ElementType::kGain, type_specific = fuchsia_hardware_audio_signalprocessing::TypeSpecificElement::gain(fuchsia_hardware_audio_signalprocessing::Gain{ type = fuchsia_hardware_audio_signalprocessing::GainType::kDecibels, min_gain = -63.5, max_gain = 0, min_gain_step = 0.5, }), }, fuchsia_hardware_audio_signalprocessing::Element{ id = 2, type = fuchsia_hardware_audio_signalprocessing::ElementType::kMute, }, ]
```

### Get the supported topologies

```posix-terminal
audio-codec-ctl t
```

```none {:.devsite-disable-click-to-copy}
Executing on device: /svc/fuchsia.hardware.audio.CodecConnectorService/default/codec_connector
[ fuchsia_hardware_audio_signalprocessing::Topology{ id = 1, processing_elements_edge_pairs = [ fuchsia_hardware_audio_signalprocessing::EdgePair{ processing_element_id_from = 1, processing_element_id_to = 2, }, fuchsia_hardware_audio_signalprocessing::EdgePair{ processing_element_id_from = 2, processing_element_id_to = 3, }, ], }, ]
```

### Get a processing element state

```posix-terminal
audio-codec-ctl w 1
```

```none {:.devsite-disable-click-to-copy}
Executing on device: /svc/fuchsia.hardware.audio.CodecConnectorService/default/codec_connector
fuchsia_hardware_audio_signalprocessing::ElementState{ type_specific = fuchsia_hardware_audio_signalprocessing::TypeSpecificElementState::gain(fuchsia_hardware_audio_signalprocessing::GainElementState{ gain = 0, }), started = true, }
```

### Control a processing element

```posix-terminal
audio-codec-ctl set 1 start gain 1.23 vendor 0x12 0x98
```

```none {:.devsite-disable-click-to-copy}
Setting element state:
fuchsia_hardware_audio_signalprocessing::SignalProcessingSetElementStateRequest{ processing_element_id = 1, state = fuchsia_hardware_audio_signalprocessing::ElementState{ type_specific = fuchsia_hardware_audio_signalprocessing::TypeSpecificElementState::gain(fuchsia_hardware_audio_signalprocessing::GainElementState{ gain = 1.23, }), started = true, vendor_specific_data = [ 18, 152, ], }, }
Executing on device: /svc/fuchsia.hardware.audio.CodecConnectorService/default/codec_connector
```

### Specify device

```posix-terminal
audio-codec-ctl -d default p
```

```none {:.devsite-disable-click-to-copy}
Executing on device: /svc/fuchsia.hardware.audio.CodecConnectorService/default/codec_connector
fuchsia_hardware_audio::PlugState{ plugged = true, plug_state_time = 1167863520, }
```

```posix-terminal
audio-codec-ctl -d non-existent p
```

```none {:.devsite-disable-click-to-copy}
Executing on device: /svc/fuchsia.hardware.audio.CodecConnectorService/non-existent/codec_connector
could not connect to:/svc/fuchsia.hardware.audio.CodecConnectorService/non-existent/codec_connector status:ZX_ERR_NOT_FOUND
```

```posix-terminal
audio-codec-ctl -d /svc/fuchsia.hardware.audio.CodecConnectorService/default/codec_connector p
```

```none {:.devsite-disable-click-to-copy}
Executing on device: /svc/fuchsia.hardware.audio.CodecConnectorService/default/codec_connector
fuchsia_hardware_audio::PlugState{ plugged = true, plug_state_time = 1167863520, }
```

### Source code {#source}

Source code for `audio-codec-ctl`: [`//src/media/audio/tools/audio-codec-ctl/`][src]

[src]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/media/audio/tools/audio-codec-ctl/

<!--

// LINT.ThenChange(//src/media/audio/tools/audio-codec-ctl/main.cc)

-->
