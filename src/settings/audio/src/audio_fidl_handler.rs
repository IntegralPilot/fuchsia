// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::audio_controller::{AudioController, Request};
use crate::types::{
    AudioError, AudioInfo, AudioSettingSource, AudioStream, AudioStreamType, SetAudioStream,
};
use async_utils::hanging_get::server;
use fidl_fuchsia_media::{AudioRenderUsage, AudioRenderUsage2};
use fidl_fuchsia_settings::{
    AudioRequest, AudioRequestStream, AudioSettings, AudioSettings2, AudioStreamSettingSource,
    AudioStreamSettings, AudioStreamSettings2, AudioWatch2Responder, AudioWatchResponder,
    Error as SettingsError, Volume,
};
use fuchsia_async as fasync;
use fuchsia_trace as ftrace;
use futures::StreamExt;
use futures::channel::mpsc::UnboundedSender;
use futures::channel::oneshot;
use settings_common::inspect::event::{
    HangingGetObserver, RequestType, ResponseType, UsagePublisher,
};
use settings_common::{trace, trace_guard};

impl From<&AudioInfo> for AudioSettings {
    fn from(info: &AudioInfo) -> Self {
        let mut streams = Vec::new();
        for stream in &info.streams {
            let stream_settings = AudioStreamSettings::try_from(*stream);
            if let Ok(stream_settings) = stream_settings {
                streams.push(stream_settings);
            }
        }

        AudioSettings { streams: Some(streams), ..Default::default() }
    }
}

impl From<&AudioInfo> for AudioSettings2 {
    fn from(info: &AudioInfo) -> Self {
        let mut streams = Vec::new();
        for stream in &info.streams {
            streams.push(AudioStreamSettings2::from(*stream));
        }

        AudioSettings2 { streams: Some(streams), ..Default::default() }
    }
}

impl TryFrom<AudioStream> for AudioStreamSettings {
    type Error = ();

    fn try_from(stream: AudioStream) -> Result<Self, ()> {
        match AudioRenderUsage::try_from(stream.stream_type) {
            Err(_) => Err(()),
            Ok(stream_type) => Ok(AudioStreamSettings {
                stream: Some(stream_type),
                source: Some(AudioStreamSettingSource::from(stream.source)),
                user_volume: Some(Volume {
                    level: Some(stream.user_volume_level),
                    muted: Some(stream.user_volume_muted),
                    ..Default::default()
                }),
                ..Default::default()
            }),
        }
    }
}

impl From<AudioStream> for AudioStreamSettings2 {
    fn from(stream: AudioStream) -> Self {
        AudioStreamSettings2 {
            stream: Some(AudioRenderUsage2::from(stream.stream_type)),
            source: Some(AudioStreamSettingSource::from(stream.source)),
            user_volume: Some(Volume {
                level: Some(stream.user_volume_level),
                muted: Some(stream.user_volume_muted),
                ..Default::default()
            }),
            ..Default::default()
        }
    }
}

impl From<AudioRenderUsage> for AudioStreamType {
    fn from(usage: AudioRenderUsage) -> Self {
        match usage {
            AudioRenderUsage::Background => AudioStreamType::Background,
            AudioRenderUsage::Communication => AudioStreamType::Communication,
            AudioRenderUsage::Interruption => AudioStreamType::Interruption,
            AudioRenderUsage::Media => AudioStreamType::Media,
            AudioRenderUsage::SystemAgent => AudioStreamType::SystemAgent,
        }
    }
}

impl TryFrom<AudioStreamType> for AudioRenderUsage {
    type Error = ();
    fn try_from(usage: AudioStreamType) -> Result<Self, Self::Error> {
        match usage {
            AudioStreamType::Accessibility => Err(()),
            AudioStreamType::Background => Ok(AudioRenderUsage::Background),
            AudioStreamType::Communication => Ok(AudioRenderUsage::Communication),
            AudioStreamType::Interruption => Ok(AudioRenderUsage::Interruption),
            AudioStreamType::Media => Ok(AudioRenderUsage::Media),
            AudioStreamType::SystemAgent => Ok(AudioRenderUsage::SystemAgent),
        }
    }
}

impl From<AudioStreamType> for AudioRenderUsage2 {
    fn from(usage: AudioStreamType) -> Self {
        match usage {
            AudioStreamType::Accessibility => AudioRenderUsage2::Accessibility,
            AudioStreamType::Background => AudioRenderUsage2::Background,
            AudioStreamType::Communication => AudioRenderUsage2::Communication,
            AudioStreamType::Interruption => AudioRenderUsage2::Interruption,
            AudioStreamType::Media => AudioRenderUsage2::Media,
            AudioStreamType::SystemAgent => AudioRenderUsage2::SystemAgent,
        }
    }
}

impl TryFrom<AudioRenderUsage2> for AudioStreamType {
    type Error = ();
    fn try_from(usage: AudioRenderUsage2) -> Result<Self, Self::Error> {
        match usage {
            AudioRenderUsage2::Accessibility => Ok(AudioStreamType::Accessibility),
            AudioRenderUsage2::Background => Ok(AudioStreamType::Background),
            AudioRenderUsage2::Communication => Ok(AudioStreamType::Communication),
            AudioRenderUsage2::Interruption => Ok(AudioStreamType::Interruption),
            AudioRenderUsage2::Media => Ok(AudioStreamType::Media),
            AudioRenderUsage2::SystemAgent => Ok(AudioStreamType::SystemAgent),
            _ => Err(()),
        }
    }
}

impl From<AudioStreamSettingSource> for AudioSettingSource {
    fn from(source: AudioStreamSettingSource) -> Self {
        match source {
            AudioStreamSettingSource::User => AudioSettingSource::User,
            AudioStreamSettingSource::System => AudioSettingSource::System,
            AudioStreamSettingSource::SystemWithFeedback => AudioSettingSource::SystemWithFeedback,
        }
    }
}

impl From<AudioSettingSource> for AudioStreamSettingSource {
    fn from(source: AudioSettingSource) -> Self {
        match source {
            AudioSettingSource::User => AudioStreamSettingSource::User,
            AudioSettingSource::System => AudioStreamSettingSource::System,
            AudioSettingSource::SystemWithFeedback => AudioStreamSettingSource::SystemWithFeedback,
        }
    }
}

// Clippy warns about all variants starting with the same prefix `No`.
#[allow(clippy::enum_variant_names)]
#[derive(thiserror::Error, Debug, PartialEq)]
enum Error {
    #[error("request has no streams")]
    NoStreams,
    #[error("missing user_volume at stream {0}")]
    NoUserVolume(usize),
    #[error("missing user_volume.level and user_volume.muted at stream {0}")]
    MissingVolumeAndMuted(usize),
    #[error("missing stream at stream {0}")]
    NoStreamType(usize),
    #[error("missing source at stream {0}")]
    NoSource(usize),
    #[error("request has an unknown stream type")]
    UnrecognizedStreamType,
}

fn to_request(settings: AudioSettings, id: ftrace::Id) -> Result<Vec<SetAudioStream>, Error> {
    trace!(id, c"to_request");
    settings
        .streams
        .map(|streams| {
            streams
                .into_iter()
                .enumerate()
                .map(|(i, stream)| {
                    let user_volume = stream.user_volume.ok_or(Error::NoUserVolume(i))?;
                    let user_volume_level = user_volume.level;
                    let user_volume_muted = user_volume.muted;
                    let stream_type = stream.stream.ok_or(Error::NoStreamType(i))?.into();
                    let source = stream.source.ok_or(Error::NoSource(i))?.into();
                    let request = SetAudioStream {
                        stream_type,
                        source,
                        user_volume_level,
                        user_volume_muted,
                    };
                    if request.is_valid_payload() {
                        Ok(request)
                    } else {
                        Err(Error::MissingVolumeAndMuted(i))
                    }
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .unwrap_or(Err(Error::NoStreams))
}

fn to_request2(settings: AudioSettings2, id: ftrace::Id) -> Result<Vec<SetAudioStream>, Error> {
    trace!(id, c"to_request2");
    settings
        .streams
        .map(|streams| {
            streams
                .into_iter()
                .enumerate()
                .map(|(i, stream)| {
                    let user_volume = stream.user_volume.ok_or(Error::NoUserVolume(i))?;
                    let user_volume_level = user_volume.level;
                    let user_volume_muted = user_volume.muted;
                    let stream_type = match stream.stream.ok_or(Error::NoStreamType(i))?.try_into()
                    {
                        Ok(stream_type) => Ok(stream_type),
                        Err(_) => Err(Error::UnrecognizedStreamType),
                    }?;
                    let source = stream.source.ok_or(Error::NoSource(i))?.into();
                    let request = SetAudioStream {
                        stream_type,
                        source,
                        user_volume_level,
                        user_volume_muted,
                    };
                    if request.is_valid_payload() {
                        Ok(request)
                    } else {
                        Err(Error::MissingVolumeAndMuted(i))
                    }
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .unwrap_or(Err(Error::NoStreams))
}

pub(crate) type SubscriberObject = HangingGetObserver<AudioInfo, AudioWatchResponder>;
type HangingGetFn = fn(&AudioInfo, SubscriberObject) -> bool;
pub(crate) type HangingGet = server::HangingGet<AudioInfo, SubscriberObject, HangingGetFn>;
pub(crate) type Publisher = server::Publisher<AudioInfo, SubscriberObject, HangingGetFn>;
pub(crate) type Subscriber = server::Subscriber<AudioInfo, SubscriberObject, HangingGetFn>;

pub(crate) type SubscriberObject2 = HangingGetObserver<AudioInfo, AudioWatch2Responder>;
type HangingGetFn2 = fn(&AudioInfo, SubscriberObject2) -> bool;
pub(crate) type HangingGet2 = server::HangingGet<AudioInfo, SubscriberObject2, HangingGetFn2>;
pub(crate) type Publisher2 = server::Publisher<AudioInfo, SubscriberObject2, HangingGetFn2>;
pub(crate) type Subscriber2 = server::Subscriber<AudioInfo, SubscriberObject2, HangingGetFn2>;

pub struct AudioFidlHandler {
    hanging_get: HangingGet,
    hanging_get2: HangingGet2,
    controller_tx: UnboundedSender<Request>,
    usage_publisher: UsagePublisher<AudioInfo>,
}

impl AudioFidlHandler {
    pub(crate) fn new(
        audio_controller: &mut AudioController,
        usage_publisher: UsagePublisher<AudioInfo>,
        controller_tx: UnboundedSender<Request>,
        initial_value: AudioInfo,
    ) -> Self {
        let hanging_get = HangingGet::new(initial_value.clone(), Self::hanging_get);
        let hanging_get2 = HangingGet2::new(initial_value, Self::hanging_get2);
        audio_controller
            .register_publishers(hanging_get.new_publisher(), hanging_get2.new_publisher());
        Self { hanging_get, hanging_get2, controller_tx, usage_publisher }
    }

    fn hanging_get(info: &AudioInfo, observer: SubscriberObject) -> bool {
        let (usage_responder, responder) = observer.into_parts();
        usage_responder.respond(format!("{info:?}"), ResponseType::OkSome);
        if let Err(e) = responder.send(&AudioSettings::from(info)) {
            log::warn!("Failed to respond to watch request: {e:?}");
            return false;
        }
        true
    }

    fn hanging_get2(info: &AudioInfo, observer: SubscriberObject2) -> bool {
        let (usage_responder, responder) = observer.into_parts();
        usage_responder.respond(format!("{info:?}"), ResponseType::OkSome);
        if let Err(e) = responder.send(&AudioSettings2::from(info)) {
            log::warn!("Failed to respond to watch request: {e:?}");
            return false;
        }
        true
    }

    pub fn handle_stream(&mut self, mut stream: AudioRequestStream) {
        let request_handler = std::rc::Rc::new(RequestHandler {
            subscriber: self.hanging_get.new_subscriber(),
            subscriber2: self.hanging_get2.new_subscriber(),
            controller_tx: self.controller_tx.clone(),
            usage_publisher: self.usage_publisher.clone(),
        });
        fasync::Task::local(async move {
            while let Some(Ok(request)) = stream.next().await {
                let request_handler = std::rc::Rc::clone(&request_handler);
                fasync::Task::local(async move {
                    request_handler.handle_request(request).await;
                })
                .detach();
            }
        })
        .detach();
    }
}

#[derive(Debug)]
enum HandlerError {
    AlreadySubscribed,
    InvalidArgument(
        // Error used by Debug impl for inspect logs.
        #[allow(dead_code)] Error,
    ),
    ControllerStopped,
    Controller(AudioError),
}

impl From<&HandlerError> for ResponseType {
    fn from(error: &HandlerError) -> Self {
        match error {
            HandlerError::AlreadySubscribed => ResponseType::AlreadySubscribed,
            HandlerError::InvalidArgument(_) => ResponseType::InvalidArgument,
            HandlerError::ControllerStopped => ResponseType::UnexpectedError,
            HandlerError::Controller(e) => ResponseType::from(e),
        }
    }
}

struct RequestHandler {
    subscriber: Subscriber,
    subscriber2: Subscriber2,
    controller_tx: UnboundedSender<Request>,
    usage_publisher: UsagePublisher<AudioInfo>,
}

impl RequestHandler {
    async fn handle_request(&self, request: AudioRequest) {
        match request {
            AudioRequest::Watch { responder } => {
                let usage_res = self.usage_publisher.request("Watch".to_string(), RequestType::Get);
                let observer = HangingGetObserver::new(usage_res, responder);
                if let Err(observer) = self.subscriber.register2(observer) {
                    let e = HandlerError::AlreadySubscribed;
                    let (usage_res, responder) = observer.into_parts();
                    usage_res.respond(format!("Err({e:?})"), ResponseType::from(&e));
                    drop(responder);
                }
            }
            AudioRequest::Watch2 { responder } => {
                let usage_res =
                    self.usage_publisher.request("Watch2".to_string(), RequestType::Get);
                let observer = HangingGetObserver::new(usage_res, responder);
                if let Err(observer) = self.subscriber2.register2(observer) {
                    let e = HandlerError::AlreadySubscribed;
                    let (usage_res, responder) = observer.into_parts();
                    usage_res.respond(format!("Err({e:?})"), ResponseType::from(&e));
                    drop(responder);
                }
            }
            AudioRequest::Set { settings, responder } => {
                let trace_id = ftrace::Id::new();
                let _guard = trace_guard!(trace_id, c"audio fidl handler set");
                let usage_res = self
                    .usage_publisher
                    .request(format!("Set{{settings:{settings:?}}}"), RequestType::Set);
                if let Err(e) = self.set(settings, trace_id).await {
                    usage_res.respond(format!("Err({e:?}"), ResponseType::from(&e));
                    let _ = responder.send(Err(SettingsError::Failed));
                } else {
                    usage_res.respond("Ok(())".to_string(), ResponseType::OkNone);
                    let _ = responder.send(Ok(()));
                }
            }
            AudioRequest::Set2 { settings, responder } => {
                let trace_id = ftrace::Id::new();
                let _guard = trace_guard!(trace_id, c"audio fidl handler set2");
                let usage_res = self
                    .usage_publisher
                    .request(format!("Set{{settings:{settings:?}}}"), RequestType::Set);
                if let Err(e) = self.set2(settings, trace_id).await {
                    usage_res.respond(format!("Err({e:?}"), ResponseType::from(&e));
                    let _ = responder.send(Err(SettingsError::Failed));
                } else {
                    usage_res.respond("Ok(())".to_string(), ResponseType::OkNone);
                    let _ = responder.send(Ok(()));
                }
            }
            _ => {
                log::error!("Unknown audio request");
            }
        }
    }

    async fn set(&self, settings: AudioSettings, trace_id: ftrace::Id) -> Result<(), HandlerError> {
        let (set_tx, set_rx) = oneshot::channel();
        let input_devices =
            to_request(settings, trace_id).map_err(HandlerError::InvalidArgument)?;
        self.controller_tx
            .unbounded_send(Request::Set(input_devices, trace_id, set_tx))
            .map_err(|_| HandlerError::ControllerStopped)?;
        set_rx
            .await
            .map_err(|_| HandlerError::ControllerStopped)
            .and_then(|res| res.map_err(HandlerError::Controller))
    }

    async fn set2(
        &self,
        settings: AudioSettings2,
        trace_id: ftrace::Id,
    ) -> Result<(), HandlerError> {
        let (set_tx, set_rx) = oneshot::channel();
        let input_devices =
            to_request2(settings, trace_id).map_err(HandlerError::InvalidArgument)?;
        self.controller_tx
            .unbounded_send(Request::Set(input_devices, trace_id, set_tx))
            .map_err(|_| HandlerError::ControllerStopped)?;
        set_rx
            .await
            .map_err(|_| HandlerError::ControllerStopped)
            .and_then(|res| res.map_err(HandlerError::Controller))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_stream() -> AudioStreamSettings {
        AudioStreamSettings {
            stream: Some(fidl_fuchsia_media::AudioRenderUsage::Media),
            source: Some(AudioStreamSettingSource::User),
            user_volume: Some(Volume {
                level: Some(0.6),
                muted: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn test_stream2() -> AudioStreamSettings2 {
        AudioStreamSettings2 {
            stream: Some(fidl_fuchsia_media::AudioRenderUsage2::Media),
            source: Some(AudioStreamSettingSource::User),
            user_volume: Some(Volume {
                level: Some(0.6),
                muted: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    // Verifies that an entirely empty settings request results in an appropriate error.
    #[fuchsia::test]
    fn test_request_from_settings_empty() {
        let id = ftrace::Id::new();
        let request = to_request(AudioSettings::default(), id);

        assert_eq!(request, Err(Error::NoStreams));
    }

    // Verifies that an entirely empty settings request2 results in an appropriate error.
    #[fuchsia::test]
    fn test_request2_from_settings_empty() {
        let id = ftrace::Id::new();
        let request = to_request2(AudioSettings2::default(), id);

        assert_eq!(request, Err(Error::NoStreams));
    }

    // Verifies that a settings request missing user volume info results in an appropriate error.
    #[fuchsia::test]
    fn test_request_missing_user_volume() {
        let mut stream = test_stream();
        stream.user_volume = None;

        let audio_settings = AudioSettings { streams: Some(vec![stream]), ..Default::default() };

        let id = ftrace::Id::new();
        let request = to_request(audio_settings, id);

        assert_eq!(request, Err(Error::NoUserVolume(0)));
    }

    // Verifies that a settings request2 missing user volume info results in an appropriate error.
    #[fuchsia::test]
    fn test_request2_missing_user_volume() {
        let mut stream = test_stream2();
        stream.user_volume = None;

        let audio_settings = AudioSettings2 { streams: Some(vec![stream]), ..Default::default() };

        let id = ftrace::Id::new();
        let request = to_request2(audio_settings, id);

        assert_eq!(request, Err(Error::NoUserVolume(0)));
    }

    // Verifies that a settings request missing the stream type results in an appropriate error.
    #[fuchsia::test]
    fn test_request_missing_stream_type() {
        let mut stream = test_stream();
        stream.stream = None;

        let audio_settings = AudioSettings { streams: Some(vec![stream]), ..Default::default() };

        let id = ftrace::Id::new();
        let request = to_request(audio_settings, id);

        assert_eq!(request, Err(Error::NoStreamType(0)));
    }

    // Verifies that a settings request2 missing the stream type results in an appropriate error.
    #[fuchsia::test]
    fn test_request2_missing_stream_type() {
        let mut stream = test_stream2();
        stream.stream = None;

        let audio_settings = AudioSettings2 { streams: Some(vec![stream]), ..Default::default() };

        let id = ftrace::Id::new();
        let request = to_request2(audio_settings, id);

        assert_eq!(request, Err(Error::NoStreamType(0)));
    }

    // Verifies that a settings request missing the source results in an appropriate error.
    #[fuchsia::test]
    fn test_request_missing_source() {
        let mut stream = test_stream();
        stream.source = None;

        let audio_settings = AudioSettings { streams: Some(vec![stream]), ..Default::default() };

        let id = ftrace::Id::new();
        let request = to_request(audio_settings, id);

        assert_eq!(request, Err(Error::NoSource(0)));
    }

    // Verifies that a settings request2 missing the source results in an appropriate error.
    #[fuchsia::test]
    fn test_request2_missing_source() {
        let mut stream = test_stream2();
        stream.source = None;

        let audio_settings = AudioSettings2 { streams: Some(vec![stream]), ..Default::default() };

        let id = ftrace::Id::new();
        let request = to_request2(audio_settings, id);

        assert_eq!(request, Err(Error::NoSource(0)));
    }

    // Verifies that a settings request missing both the user volume level and mute state results in
    // an appropriate error.
    #[fuchsia::test]
    fn test_request_missing_user_volume_level_and_muted() {
        let mut stream = test_stream();
        stream.user_volume = Some(Volume { level: None, muted: None, ..Default::default() });

        let audio_settings = AudioSettings { streams: Some(vec![stream]), ..Default::default() };

        let id = ftrace::Id::new();
        let request = to_request(audio_settings, id);

        assert_eq!(request, Err(Error::MissingVolumeAndMuted(0)));
    }

    // Verifies that a settings request2 missing both the user volume level and mute state results in
    // an appropriate error.
    #[fuchsia::test]
    fn test_request2_missing_user_volume_level_and_muted() {
        let mut stream = test_stream2();
        stream.user_volume = Some(Volume { level: None, muted: None, ..Default::default() });

        let audio_settings = AudioSettings2 { streams: Some(vec![stream]), ..Default::default() };

        let id = ftrace::Id::new();
        let request = to_request2(audio_settings, id);

        assert_eq!(request, Err(Error::MissingVolumeAndMuted(0)));
    }
}
