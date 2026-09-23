// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Media modules:
//!
//! Sources implement the bt_a2dp::media_task interfaces to encode and send packets to a peer
//!  - Sources may require a stream_builder to generate audio in-band
//! Sinks implement the bt_a2dp::media_task interfaces to decode and present packets locally.

use fidl_fuchsia_bluetooth_bredr as bredr;
use fuchsia_bluetooth::types::Uuid;

pub mod encoding_source;
pub mod offload_source;
pub mod player;
pub mod player_sink;

pub const AUDIO_SOURCE_UUID: Uuid =
    Uuid::new16(bredr::ServiceClassProfileIdentifier::AudioSource.into_primitive());

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum AudioSourceType {
    AudioOut,
    BigBen,
    Offload,
}

impl core::fmt::Display for AudioSourceType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        write!(
            f,
            "{}",
            match self {
                AudioSourceType::AudioOut => "audio_out",
                AudioSourceType::BigBen => "big_ben",
                AudioSourceType::Offload => "offload",
            }
        )
    }
}

impl std::str::FromStr for AudioSourceType {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "audio_out" => Ok(AudioSourceType::AudioOut),
            "big_ben" => Ok(AudioSourceType::BigBen),
            "offload" => Ok(AudioSourceType::Offload),
            _ => Err(anyhow::format_err!(
                "Unrecognized audio source '{s}', use audio_out, big_ben, or offload"
            )),
        }
    }
}
