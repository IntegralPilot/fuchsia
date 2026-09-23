// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_audio_sub_command::SubCommand;
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "audio",
    description = "Interact with the audio subsystem.",
    example = "\
    Generate a 5s long audio signal with 440Hz frequency and write to a file: \n\
    \t$ ffx audio gen sine --frequency 440 --duration 5s --format 48000,uint8,1ch > ~/sine.wav \n\n\
    Play a wav file using audio_core API: \n\
    \t$ cat ~/sine.wav | ffx audio play \n\n\
    Record audio signal to a file using audio_core API: \n\
    \t$ ffx audio record --duration 1s --format 48000,uint8,1ch > ~/recording.wav\n\n\
    List available audio devices on target: \n\
    \t$ ffx audio device list\n\n\
    Print information about a specific audio device on target: \n\
    \t$ ffx audio device --name 3d99d780 info\n\n\
    Play a wav file directly to device hardware: \n\
    \t$ cat ~/sine.wav | ffx audio device play\n",
    note = "Format parameters: \
    Some commands take a '--format <format>' argument for describing audio PCM format.\n\
    The <format> argument has the pattern: <SampleRate>,<SampleType>,<Channels>
        SampleRate: Integer
        SampleType options: uint8, int16, int32, float32
        Channels: <uint>ch

    example: --format 48000,float32,2ch"
)]
pub struct AudioCommand {
    #[argh(subcommand)]
    pub subcommand: SubCommand,
}
