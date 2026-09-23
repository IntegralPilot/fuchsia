// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_media_sessions2::{ContentType, Error, PlayerState, PlayerStatus, RepeatMode};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub enum SerializablePlayerState {
    Idle = 0,
    Playing = 1,
    Paused = 2,
    Buffering = 3,
    Error = 4,
}

/// Wrap the FIDL PlayerState as a serializable object.
impl From<PlayerState> for SerializablePlayerState {
    fn from(ps: PlayerState) -> Self {
        match ps {
            PlayerState::Idle => Self::Idle,
            PlayerState::Playing => Self::Playing,
            PlayerState::Paused => Self::Paused,
            PlayerState::Buffering => Self::Buffering,
            PlayerState::Error => Self::Error,
        }
    }
}

#[derive(Debug, Serialize)]
pub enum SerializableRepeatMode {
    Off = 0,
    Group = 1,
    Single = 2,
}

/// Wrap the FIDL RepeatMode as a serializable object.
impl From<RepeatMode> for SerializableRepeatMode {
    fn from(m: RepeatMode) -> Self {
        match m {
            RepeatMode::Off => Self::Off,
            RepeatMode::Group => Self::Group,
            RepeatMode::Single => Self::Single,
        }
    }
}

#[derive(Debug, Serialize)]
pub enum SerializableContentType {
    Other = 1,
    Audio = 2,
    Video = 3,
    Music = 4,
    TvShow = 5,
    Movie = 6,
}

/// Wrap the FIDL ContentType as a serializable object.
impl From<ContentType> for SerializableContentType {
    fn from(ct: ContentType) -> Self {
        match ct {
            ContentType::Other => Self::Other,
            ContentType::Audio => Self::Audio,
            ContentType::Video => Self::Video,
            ContentType::Music => Self::Music,
            ContentType::TvShow => Self::TvShow,
            ContentType::Movie => Self::Movie,
        }
    }
}

#[derive(Debug, Serialize)]
pub enum SerializableError {
    Other = 1,
}

/// Wrap the FIDL Error as a serializable object.
impl From<Error> for SerializableError {
    fn from(e: Error) -> Self {
        match e {
            Error::Other => Self::Other,
        }
    }
}

#[derive(Debug, Default, Serialize)]
pub struct PlayerStatusWrapper {
    pub duration: Option<i64>,
    pub player_state: Option<SerializablePlayerState>,
    pub repeat_mode: Option<SerializableRepeatMode>,
    pub shuffle_on: Option<bool>,
    pub content_type: Option<SerializableContentType>,
    pub error: Option<SerializableError>,
    pub is_live: Option<bool>,
}

/// Wrap the FIDL PlayerStatus as a serializable object.
/// The data "timeline_function"(Option<fidl_fuchsia_media::TimelineFunction>)
/// is abandoned here to simplify it since it's not used for the test
/// environment.
impl From<Option<PlayerStatus>> for PlayerStatusWrapper {
    fn from(player_stat: Option<PlayerStatus>) -> Self {
        let Some(ps) = player_stat else { return Self::default() };
        Self {
            duration: ps.duration,
            player_state: ps.player_state.map(Into::into),
            repeat_mode: ps.repeat_mode.map(Into::into),
            shuffle_on: ps.shuffle_on,
            content_type: ps.content_type.map(Into::into),
            error: ps.error.map(Into::into),
            is_live: ps.is_live,
        }
    }
}
