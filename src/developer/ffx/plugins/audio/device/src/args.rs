// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use fdomain_fuchsia_audio_device as fadevice;
use ffx_core::ffx_command;
use fuchsia_audio_fdomain::Format;
use fuchsia_audio_fdomain::dai::DaiFormat;
use fuchsia_audio_fdomain::device::{Direction, HardwareType};

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "device",
    description = "Interact directly with device hardware. \
                   The flags on the device command filter the list of available devices. \
                   Commands that operate on a single device pick the first matching device, after \
                   filtering. \
                   For example, if the target has only one device, and no filter flags are \
                   provided, `ffx audio device` will use that device by default.",
    example = "Show information about a specific device:

    $ ffx audio device --name 3d99d780 --direction input info

Show information about a specific device in the audio device registry:

    $ ffx audio device --token-id 1 info

Play a WAV file directly to device hardware:

    $ cat ~/sine.wav | ffx audio device --name a70075f2 play
    $ ffx audio device --name a70075f2 play --file ~/sine.wav

Record a WAV file directly from device hardware:

    $ ffx audio device --name 3d99d780 record --format 48000,uint8,1ch --duration 1s

Mute the stream of an output device:

    $ ffx audio device --name a70075f2 --direction output mute

Set the gain of an output device to -20 dB:

    $ ffx audio device --name a70075f2 --direction output gain -20

Turn AGC on for an input device:

    $ ffx audio device --name 3d99d780 --direction input agc on"
)]
pub struct DeviceCommand {
    #[argh(subcommand)]
    pub subcommand: SubCommand,

    #[argh(
        option,
        description = "device devfs node name. \
        e.g. 3d99d780 for the devfs path /dev/class/audio-input/3d99d780."
    )]
    pub name: Option<String>,

    #[argh(
        option,
        description = "device token ID, for a device in the audio device registry. \
        Only applies if the audio device registry is available on the target."
    )]
    pub token_id: Option<fadevice::TokenId>,

    #[argh(
        option,
        long = "direction",
        description = "device direction. Accepted values: input, output. \
        Play and record will use output and input respectively by default."
    )]
    pub device_direction: Option<Direction>,

    #[argh(
        option,
        long = "type",
        description = "device type. Accepted values: StreamConfig, Composite."
    )]
    pub device_type: Option<HardwareType>,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand)]
pub enum SubCommand {
    List(ListCommand),
    Info(InfoCommand),
    Play(PlayCommand),
    Record(RecordCommand),
    Gain(GainCommand),
    Mute(MuteCommand),
    Unmute(UnmuteCommmand),
    Agc(AgcCommand),
    Set(SetCommand),
    Start(StartCommand),
    Stop(StopCommand),
    Reset(ResetCommand),
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "list",
    description = "Lists audio devices.",
    example = "ffx audio device --type StreamConfig list"
)]
pub struct ListCommand {}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "info",
    description = "Show information about a specific audio device.",
    example = "ffx audio device --type StreamConfig --direction input info"
)]
pub struct InfoCommand {}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "play", description = "Send audio data directly to device ring buffer.")]
pub struct PlayCommand {
    #[argh(
        option,
        description = "file in WAV format containing audio signal. \
        If not specified, ffx command will read from stdin."
    )]
    pub file: Option<String>,

    #[argh(
        option,
        description = "signal processing element ID, for an Endpoint element of type RingBuffer"
    )]
    pub element_id: Option<fadevice::ElementId>,

    #[argh(
        option,
        description = "bitmask of channels to activate (deactivate the others). \
If not specified, activate all channels present in the playback format"
    )]
    pub channels: Option<u64>,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "record", description = "Capture audio data directly from ring buffer.")]
pub struct RecordCommand {
    #[argh(
        option,
        description = "duration of output signal. Examples: 5ms or 3s. \
        If not specified, press ENTER to stop recording.",
        from_str_fn(parse_duration)
    )]
    pub duration: Option<std::time::Duration>,

    #[argh(option, description = "output format (see 'ffx audio help' for more information).")]
    pub format: Format,

    #[argh(
        option,
        description = "signal processing element ID, for an Endpoint element of type RingBuffer"
    )]
    pub element_id: Option<fadevice::ElementId>,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "gain",
    description = "Request to set the gain of the stream, in decibels."
)]
pub struct GainCommand {
    #[argh(option, description = "gain, in decibels, to set the stream to.")]
    pub gain: f32,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "mute", description = "Request to mute a stream.")]
pub struct MuteCommand {}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "unmute", description = "Request to unmute a stream.")]
pub struct UnmuteCommmand {}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "agc",
    description = "Request to enable or disable automatic gain control for the stream."
)]
pub struct AgcCommand {
    #[argh(
        positional,
        description = "enable or disable AGC. Accepted values: on, off",
        from_str_fn(string_to_enable)
    )]
    pub enable: bool,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "set", description = "Set a device property.")]
pub struct SetCommand {
    #[argh(subcommand)]
    pub subcommand: SetSubCommand,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand)]
pub enum SetSubCommand {
    DaiFormat(SetDaiFormatCommand),
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "dai-format",
    description = "Set the DAI format of device or signal processing element.",
    example = "Set the DAI format for a specific Dai Endpoint signal processing element:

    $ ffx audio device set dai-format --element-id 2 48000,2ch,0x3,pcm_signed,16in16,i2s",
    note = r"This command accepts a DAI format as a comma separated string:

<FrameRate>,<NumChannels>,<ChannelsToUseBitmask>,<DaiSampleFormat>,<SampleSize>,<DaiFrameFormat>

Where:
    <FrameRate>: integer frame rate in Hz, e.g. 48000
    <NumChannels>ch: number of channels, e.g. 2ch
    <ChannelsToUseBitmask>: bitmask for channels that are in use,
        as a hexadecimal number prefixed with 0x,
        e.g. 0x3 for the first two channels, usually stereo left/right
    <DaiSampleFormat>: sample format, one of:
        pdm, pcm_signed, pcm_unsigned, pcm_float
    <SampleSize>: sample and slot size in bits as: <ValidBits>in<TotalBits>
        e.g. 16in32 for 16 valid sample bits in a 32 bit slot
    <DaiFrameFormat>: frame format, either:
        a standard format, one of:
            none, i2s, stereo_left, stereo_right, tdm1, tdm2, tdm3
        or a custom format:
                custom:<Justification>;<Clocking>;<FrameSyncOffset>;<FrameSyncSize>
            where:
            <Justification>: justification of samples within a slot, one of:
                left_justified, right_justified
            <Clocking>: clocking of data samples, one of:
                raising_sclk, falling_sclk
            <FrameSyncOffset>: number of sclks between the beginning of a
                frame sync change and audio samples.
                e.g. 1 for i2s, 0 for left justified
            <FrameSyncSize>: number of sclks that the frame sync is high.
                e.g. 1

Examples:
    48000,2ch,0x3,pcm_signed,16in32,i2s
    96000,1ch,0x1,pcm_float,32in32,custom:right_justified;falling_sclk;-1;0"
)]
pub struct SetDaiFormatCommand {
    #[argh(positional, description = "DAI format")]
    pub format: DaiFormat,

    #[argh(
        option,
        description = "signal processing element ID, \
        for an Endpoint element of type Dai"
    )]
    pub element_id: Option<fadevice::ElementId>,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "start", description = "Start device hardware.")]
pub struct StartCommand {}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "stop", description = "Stop device hardware.")]
pub struct StopCommand {}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "reset", description = "Reset device hardware.")]
pub struct ResetCommand {}

fn string_to_enable(value: &str) -> Result<bool, String> {
    if value == "on" {
        Ok(true)
    } else if value == "off" {
        Ok(false)
    } else {
        Err(format!("Expected one of: on, off"))
    }
}

fn parse_duration(value: &str) -> Result<std::time::Duration, String> {
    fuchsia_audio_fdomain::parse_duration(value)
}
