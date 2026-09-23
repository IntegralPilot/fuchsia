// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_media_sounds::{PlaySoundError, PlayerRequest, PlayerRequestStream};
use fuchsia_component_test::{ChildOptions, RealmBuilder};
use futures::TryStreamExt;
use futures::channel::mpsc::UnboundedSender;

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum SoundPlayerBehavior {
    /// All operations should succeed.
    Succeed,
    /// Adding the sound should fail. This means playing the sound will also fail.
    FailAddSound,
    /// Only playing the sound should fail.
    FailPlaySound,
}

#[derive(Debug, PartialEq)]
pub(crate) enum SoundPlayerRequestName {
    AddSoundFromFile,
    PlaySound2,
}

/// A mock implementation of `fuchsia.media.sounds.Player`, which
/// a) handles requests per the specified `SoundPlayerBehavior`, and
/// b) informs an `UnboundedSender` when each requests comes in.
///
/// All `clone()`s of this mock will relay their requests to the
/// same `UnbounderSender`.
#[derive(Clone)]
pub(crate) struct SoundPlayerMock {
    name: String,
    behavior: SoundPlayerBehavior,
    request_relay_write_end: Option<UnboundedSender<SoundPlayerRequestName>>,
}

impl SoundPlayerMock {
    pub(crate) fn new<M: Into<String>>(
        name: M,
        behavior: SoundPlayerBehavior,
        request_relay_write_end: Option<UnboundedSender<SoundPlayerRequestName>>,
    ) -> Self {
        Self { name: name.into(), behavior, request_relay_write_end }
    }

    fn add_sound_from_file(&self) -> Result<i64, i32> {
        const DURATION: i64 = 42;
        if let Some(relay) = self.request_relay_write_end.as_ref() {
            relay.unbounded_send(SoundPlayerRequestName::AddSoundFromFile).unwrap();
        }
        match self.behavior {
            SoundPlayerBehavior::FailAddSound => Err(-1),
            _ => Ok(DURATION),
        }
    }

    fn play_sound2(&self) -> Result<(), PlaySoundError> {
        if let Some(relay) = self.request_relay_write_end.as_ref() {
            relay.unbounded_send(SoundPlayerRequestName::PlaySound2).unwrap();
        }
        match self.behavior {
            SoundPlayerBehavior::FailPlaySound => Err(PlaySoundError::NoSuchSound),
            _ => Ok(()),
        }
    }

    async fn serve_one_client(self, mut request_stream: PlayerRequestStream) {
        while let Some(request) =
            request_stream.try_next().await.expect("Failed to read PlayerRequest")
        {
            match request {
                PlayerRequest::AddSoundFromFile { responder, .. } => {
                    responder.send(self.add_sound_from_file()).unwrap();
                }
                PlayerRequest::PlaySound2 { responder, .. } => {
                    responder.send(self.play_sound2()).unwrap();
                }
                _ => panic!("Unexpected {:?}", request),
            };
        }
    }
}

impl_test_realm_component!(SoundPlayerMock);
