// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Fuchsia Audio Device Library
//!
//! Provides a method to create audio devices that are backed by software in Fuchsia.
//!
//! SoftStreamConfig creates a StreamConfig client that is suitable for adding to
//! [[fuchsia.media.AudioDeviceEnumerator]] via AddDeviceByChannel or
//! [[fuchsia.audio.device.Provider]] via AddDevice. It produces either an
//! AudioFrameStream (for output audio) or an AudioFrameSink (for input audio)
//!
//! SoftCodec creates a Codec client suitable for adding to [[fuchsia.audio.device.Provider]]
//! via AddCodec.  It provides a CodecDevice to capture and respond to control events from Media,
//! and shutdown the Codec (retrieving a new Codec client which can be used to re-add the Codec)

#![recursion_limit = "256"]

pub use crate::types::{AudioStreamItem, Error, Result};

/// Generic types
#[macro_use]
mod types;

/// Software Stream Config Audio Input/Output
pub mod stream_config;

/// Audio Frame Stream (output stream)
/// Produces audio packets as if it was an audio output using a [`stream_config::SoftStreamConfig`]
pub mod audio_frame_stream;

pub use audio_frame_stream::AudioFrameStream;

/// Audio Frame Sink (input sink)
/// Acts as a microphone or audio input, accepting audio packets using a
/// [`stream_config::SoftStreamConfig`]
pub mod audio_frame_sink;

pub use audio_frame_sink::AudioFrameSink;

/// Software Codec Audio Input/Output
pub mod codec;

/// Frame VMO Helper module
mod frame_vmo;
