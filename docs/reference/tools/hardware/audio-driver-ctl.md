<!--

// LINT.IfChange

-->

# audio-driver-ctl

Play, record, and configure audio streams.

## Important
`audio-driver-ctl` is deprecated. Please use `ffx audio device` tool instead. For more information,
run `ffx audio device --help` from your host machine and see
README for `ffx audio`: [`//src/developer/ffx/plugins/audio/README.md`][ffx-audio-readme]

[ffx-audio-readme]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/developer/ffx/plugins/audio/README.md

## Usage {#usage}

```none
audio-driver-ctl [-d <device>] [-t {input|output}] agc {on|off}

audio-driver-ctl [-a <mask>] [-b {8|16|20|24|32}] [-c <channels>] \
    [-d <device>] [-r <hertz>] duplex <playpath> <recordpath>

audio-driver-ctl [-d <device>] [-t {input|output}] gain <decibels>

audio-driver-ctl [-d <device>] [-t {input|output}] info

audio-driver-ctl list

audio-driver-ctl [-a <mask>] [-b {8|16|20|24|32}] [-c <channels>] \
    [-d <device>] loop <playpath>

audio-driver-ctl [-d <device>] [-t {input|output}] mute

audio-driver-ctl [-a <mask>] [-b {8|16|20|24|32}] [-c <channels>] \
    [-d <device>] [-r <hertz>] noise [<seconds>] [<amplitude>]

audio-driver-ctl [-a <mask>] [-b {8|16|20|24|32}] [-c <channels>] \
    [-d <device>] play <playpath>

audio-driver-ctl [-d <device>] [-t {input|output}] pmon [<seconds>]

audio-driver-ctl [-a <mask>] [-b {8|16|20|24|32}] [-c <channels>] \
    [-d <device>] [-r <hertz>] record <recordpath> [<seconds>]

audio-driver-ctl [-a <mask>] [-b {8|16|20|24|32}] [-c <channels>] \
    [-d <device>] [-r <hertz>] tone [<frequency>] [<seconds>] [<amplitude>]

audio-driver-ctl [-d <device>] [-t {input|output}] unmute
```

## Options {#options}

### `-a <mask>` option {#a}

Active channel mask. For example `0xf` or `15` for channels 0, 1, 2, and 3.
Defaults to all channels.

### `-b {8|16|20|24|32}` option {#b}

Bits per sample. Defaults to `16`.

### `-c <channels>` option {#c}

Number of channels to use when recording or generating tones/noises.
Does not affect WAV file playback because WAV files specify how many
channels to use in their headers. Defaults to the first driver-reported
value. Run [`info`](#info) to see how many channels your target Fuchsia device
has. The number of channels must match what the audio driver expects
because `audio-driver-ctl` does not do any mixing.

### `-d <device>` option {#d}

The device path or service instance name. If unspecified, the tool picks the first
device found. If it does not contain `/`, the tool treats it as a service instance
name (for example, `default` -> `/svc/.../default/stream_config_connector`).

### `-t {input|output}` option {#t}

The device type. Defaults to `output`. This option is ignored for commands like
[`play`](#play) that only make sense for one of the types.

### `-r <hertz>` option {#r}

The frame rate in hertz. Defaults to `48000`.

## Commands {#commands}

### `agc` command {#agc}

```none
audio-driver-ctl [-d <device>] [-t {input|output}] agc {on|off}
```

Enables or disables automatic gain control for the stream.

### `duplex` command {#duplex}

```none
audio-driver-ctl [-a <mask>] [-b {8|16|20|24|32}] [-c <channels>] \
    [-d <device>] [-r <hertz>] duplex <playpath> <recordpath>
```

Simultaneously plays the WAV file located at `<playpath>` and records
another WAV file into `<recordpath>` to analyze delays in the
system. If provided, the `-c` option applies to the recording side,
because the WAV file header determines the number of channels for playback. For duplex
mode, the `-d` parameter must be an instance name and cannot be a full path.

### `gain` command {#gain}

```none
audio-driver-ctl [-d <device>] [-t {input|output}] gain <decibels>
```

Sets the gain of the stream in decibels.

### `info` command {#info}

```none
audio-driver-ctl [-d <device>] [-t {input|output}] info
```

Gets capability and status info for a stream.

### `list` command {#list}

```none
audio-driver-ctl list
```

Lists all available input and output devices.

### `loop` command {#loop}

```none
audio-driver-ctl [-a <mask>] [-b {8|16|20|24|32}] [-c <channels>] \
    [-d <device>] loop <playpath>
```

Repeatedly plays the WAV file at `<playpath>` on the selected output until a key
is pressed.

### `mute` command {#mute}

```none
audio-driver-ctl [-d <device>] [-t {input|output}] mute
```

Mutes a stream.

### `noise` command {#noise}

```none
audio-driver-ctl [-a <mask>] [-b {8|16|20|24|32}] [-c <channels>] \
    [-d <device>] [-r <hertz>] noise [<seconds>] [<amplitude>]
```

Plays pseudo-white noise. `<seconds>` controls how long the noise plays and must
be at least `0.001` seconds. If `<seconds>` is not provided the noise plays until
a key is pressed.

### `play` command {#play}

```none
audio-driver-ctl [-a <mask>] [-b {8|16|20|24|32}] [-c <channels>] \
    [-d <device>] play <playpath>
```

Plays a WAV file.

### `pmon` command {#pmon}

```none
audio-driver-ctl [-d <device>] [-t {input|output}] pmon [<seconds>]
```

Monitors the plug state of a stream. `<seconds>` must be above `0.5` seconds
(default: `10.0` seconds).

### `record` command {#record}

```none
audio-driver-ctl [-a <mask>] [-b {8|16|20|24|32}] [-c <channels>] \
    [-d <device>] [-r <hertz>] record <recordpath> [<seconds>]
```

Records to the specified WAV file from the selected input. If `<seconds>` is not
provided the input is recorded until a key is pressed.

### `tone` command {#tone}

```none
audio-driver-ctl [-a <mask>] [-b {8|16|20|24|32}] [-c <channels>] \
    [-d <device>] [-r <hertz>] tone [<frequency>] [<seconds>] [<amplitude>]
```

Plays a sinusoidal tone. `<frequency>` must be between `15.0` and `96000.0` hertz
(default: `440.0` hertz). `<seconds>` must be above `0.001` seconds. If <seconds> is
not provided the tone plays until a key is pressed. `<amplitude>` scales the
output if provided and must be an increment of 0.1 between `0.1` and `1.0`.

### `unmute` command {#unmute}

```none
audio-driver-ctl [-d <device>] [-t {input|output}] unmute
```

Unmutes a stream. Note that the gain of the stream will be reset to its default
value.

## Examples {#examples}

### Enable automatic gain control on a stream {#examples-agc}

```posix-terminal
audio-driver-ctl agc on
```

### Get stream info {#examples-info}

This command is equivalent to `audio-driver-ctl -t output -d default info`:

```posix-terminal
audio-driver-ctl info
```

```none {:.devsite-disable-click-to-copy}
Info for audio output at "/svc/fuchsia.hardware.audio.StreamConfigConnectorOutputService/default/stream_config_connector"
  Unique ID    : 0100000000000000-0000000000000000
  Manufacturer : Spacely Sprockets
  Product      : acme
  Current Gain : 0.00 dB (unmuted, AGC on)
  Gain Caps    : gain range [-103.00, 24.00] in 0.50 dB steps; can mute; can AGC
  Plug State   : plugged
  Plug Time    : 12297829382473034410
  PD Caps      : hardwired
Number of channels      : 1
Frame rate              : 8000Hz
Bits per channel        : 16
Valid bits per channel  : 16
...
```

### List all available input and output devices {#examples-list}

```posix-terminal
audio-driver-ctl list
```

```none {:.devsite-disable-click-to-copy}
Input Devices:
  default
Output Devices:
  default
```

### Set gain of a stream to -40 decibels {#examples-gain}

This command is equivalent to `audio-driver-ctl -t output -d default gain -40`:

```posix-terminal
audio-driver-ctl gain -40
```

### Mute a stream {#examples-mute}

This command is equivalent to `audio-driver-ctl -t output -d default mute`:

```posix-terminal
audio-driver-ctl mute
```

### Repeatedly play (loop) a WAV file on a stream {#examples-loop}

This command is equivalent to `audio-driver-ctl -t output -d default loop /tmp/test.wav`:

```posix-terminal
audio-driver-ctl loop /tmp/test.wav
```

```none {:.devsite-disable-click-to-copy}
Looping /tmp/test.wav until a key is pressed
```

### Play a WAV file once on a stream {#examples-play}

This command is equivalent to `audio-driver-ctl -t output -d default play /tmp/test.wav`:

```posix-terminal
audio-driver-ctl play /tmp/test.wav
```

### Play a 450 hertz tone for 1 second at 50% amplitude on a stream {#examples-tone}

This command is equivalent to `audio-driver-ctl -t output -d default tone 450 1 0.5`:

```posix-terminal
audio-driver-ctl tone 450 1 0.5
```

```none {:.devsite-disable-click-to-copy}
Playing 450.00 Hz tone for 1.00 seconds at 0.50 amplitude
```

### Unmute a stream {#examples-unmute}

This command is equivalent to `audio-driver-ctl -t output -d default unmute`:

```posix-terminal
audio-driver-ctl unmute
```

## Notes {#notes}

<<./_access.md>>

### Supported builds for commands that exercise streams {#builds}

Commands that exercise audio streams such as [`play`](#play) are only supported
in diagnostic [product bundles][glossary.product-bundle] like `core`.
In other builds only the informational commands like `info` are supported.

### Copying WAV files between a host and a target Fuchsia device {#copy}

To copy WAV files from your host to your target Fuchsia device or
vice versa, run `fx cp (--to-target|--to-host) <source> <destination>`
on your host. `<source>` is the file you want to copy and `<destination>`
is where you want to put the copied file.

Example of copying from host to target Fuchsia device:

```posix-terminal
fx cp --to-target /path/on/host/source.wav /path/on/target/destination.wav
```

Example of copying from target Fuchsia device to host:

```posix-terminal
fx cp --to-host /path/on/target/source.wav /path/on/host/destination.wav
```

Both commands should be run from your host, not the target Fuchsia device.

### Source code {#source}

Source code for `audio-driver-ctl`: [`//src/media/audio/tools/audio-driver-ctl/`][src]

[src]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/media/audio/tools/audio-driver-ctl/

<!--

// LINT.ThenChange(//src/media/audio/tools/audio-driver-ctl/audio.cc)

-->
