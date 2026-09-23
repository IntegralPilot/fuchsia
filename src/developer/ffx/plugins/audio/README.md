# `ffx audio`

## Usage

### Generate audio signals

Use `ffx audio gen <signal-type>` to generate an audio signal of a given type:
`sine`, `square`, `triangle`, `sawtooth`, `pink-noise`, or `white-noise`.

This command outputs audio in WAV format to standard out. This can be redirected
to a file or piped directly into commands like `ffx audio play` or `ffx audio
device play`.

```posix-terminal
ffx audio gen <signal-type> --duration 5ms --frequency 440 --amplitude 0.5 --format 48000,int16,2ch
```

### Interact with `audio_core`

The `ffx audio play` and `ffx audio record` commands render and capture audio
from the `audio_core` `AudioRenderer` and `AudioCapturer` APIs, respectively.

These commands accept options to set usage, gain, mute, clock, and buffer size
for the shared VMO between the `audio_ffx_daemon` on target and the
`AudioRenderer`/`AudioCapturer`. See `ffx audio play --help` and
`ffx audio record --help` for details.

Tip: To use the ultrasound renderer and capturer, use the option `--usage
ultrasound`.

```posix-terminal
ffx audio play
ffx audio play --file ~/path/to/file.wav
ffx audio record
ffx audio play --usage ultrasound
ffx audio record --usage ultrasound
```

### Interact with attached devices

Use `ffx audio device list` to see which devices are connected. The device name
field can be used to specify a device for other `ffx audio device` commands.

```posix-terminal
ffx audio device list
```

Filter the devices with optional flags. See `ffx audio device help` for
more information about filter flags.

Specify `--name <name>` to select a device by name:

```
ffx audio device --name 3d99d780
```

Specify `--token-id <id>` to select a device by audio device registry (ADR)
token ID:

```
ffx audio device --token-id 1
```

Specify `--direction {input,output}` to select a device by direction.
This only applies to StreamConfig devices.

```
ffx audio device --direction input
```

`ffx audio device` subcommands that operate on a single device (all except `list`)
choose the first device in the list, after filtering.

Use `ffx audio device info` to print additional information about a specific
device.

```posix-terminal
ffx audio device --name <name> info
```

Use `ffx audio device play` and `ffx audio device record` to play or record an
audio signal. from the hardware ring buffer.

Note: These commands communicate directly with the device. To avoid conflicts,
the target build should not have `audio_core` present.

```posix-terminal
ffx audio device --name <name> record
```

```posix-terminal
ffx audio device --name <name> play
```

```posix-terminal
ffx audio device --name <name> play --file ~/path/to/file.wav
```

### Other Tips

* Run play and record commands at the same time to verify that a signal is
  rendered and captured correctly.

  Verify that the output of the record command matches what is expected from
  the play command.

* Play a looped WAV file:

  Use `ffmpeg` to output a looped audio signal for a given WAV file to standard
  out, then pipe it to `ffx audio device play`:

  ```posix-terminal
  ffmpeg -stream_loop <N> -i <filename> -c copy -f wav -
  ```

  Where:

  * `<N>` is the number of times to loop, or -1 to loop infinitely
  * `<filename>` is the input WAV file
  * The file `-` outputs to stdout

  The following command infinitely loops a WAV file and sends it to the device
  ring buffer:

  ```posix-terminal
  ffmpeg -stream_loop -1 -i <filename> -c copy -f wav - | ffx audio device play
  ```

## Architecture

### `ffx audio`

`ffx audio` commands, except for `gen`, interact with the target through the
[fuchsia.audio.controller][fidl-fuchsia-audio-controller] FIDL protocols, served
by the `audio_ffx_daemon` component.

### `audio_ffx_daemon`

[`audio_ffx_daemon`][ffxdaemon] is a Fuchsia component that proxies audio
commands and data from `ffx audio` commands to audio devices and `audio_core`.

The `audio_core` APIs and device ring buffers use Zircon virtual memory objects
(VMOs) that are not accessible to ffx plugins. `audio_ffx_daemon` provides
a Zircon socket interface to transfer audio data between the host and target,
and executes audio commands on behalf of the ffx client.

The `audio_dev_support` assembly input bundle contains the daemon and is enabled
only for `eng` build types.

[fidl-fuchsia-audio-controller]: //sdk/fidl/fuchsia.audio.controller
[ffxdaemon]: //src/media/audio/services/ffxdaemon
