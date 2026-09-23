// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mock_audio_core_service::audio_core_service_mock;
use anyhow::{format_err, Error};
use fidl::endpoints::DiscoverableProtocolMarker;
use fidl_fuchsia_media::{
    AudioCoreMarker, AudioRenderUsage, AudioRenderUsage2, RENDER_USAGE2_COUNT,
};
use fidl_fuchsia_settings::{
    AudioMarker, AudioProxy, AudioStreamSettingSource, AudioStreamSettings, AudioStreamSettings2,
    Volume,
};
use fuchsia_component_test::{
    Capability, ChildOptions, LocalComponentHandles, RealmBuilder, RealmInstance, Ref, Route,
};
use fuchsia_sync::RwLock;
use futures::channel::mpsc::{Receiver, Sender};
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;

pub(crate) const DEFAULT_VOLUME_LEVEL: f32 = 0.5;
pub(crate) const DEFAULT_VOLUME_MUTED: bool = false;

#[cfg(test)]
pub(crate) fn default_media_stream_settings() -> AudioStreamSettings {
    AudioStreamSettings {
        stream: Some(AudioRenderUsage::Media),
        source: Some(AudioStreamSettingSource::User),
        user_volume: Some(Volume {
            level: Some(DEFAULT_VOLUME_LEVEL),
            muted: Some(DEFAULT_VOLUME_MUTED),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn default_streams2() -> [AudioStreamSettings2; RENDER_USAGE2_COUNT as usize] {
    [
        create_default_audio_stream2(AudioRenderUsage2::Background),
        create_default_audio_stream2(AudioRenderUsage2::Media),
        create_default_audio_stream2(AudioRenderUsage2::Interruption),
        create_default_audio_stream2(AudioRenderUsage2::SystemAgent),
        create_default_audio_stream2(AudioRenderUsage2::Communication),
        create_default_audio_stream2(AudioRenderUsage2::Accessibility),
    ]
}

fn create_default_audio_stream2(usage: AudioRenderUsage2) -> AudioStreamSettings2 {
    AudioStreamSettings2 {
        stream: Some(usage),
        source: Some(AudioStreamSettingSource::User),
        user_volume: Some(Volume {
            level: Some(DEFAULT_VOLUME_LEVEL),
            muted: Some(DEFAULT_VOLUME_MUTED),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Info about an incoming request emitted by the audio core mock whenever it receives a request.
#[derive(PartialEq, Debug)]
pub(crate) enum AudioCoreRequest {
    SetVolume(AudioRenderUsage2, f32),
    SetMute(AudioRenderUsage2, bool),
}

const COMPONENT_URL: &str = "#meta/setui_service.cm";

pub(crate) struct AudioTest {
    /// The test realm the integration tests will use.
    realm: RealmInstance,
    /// The receiver end of a channel that is written to when the core audio
    /// mock processes an event.
    request_receiver: Receiver<AudioCoreRequest>,
    /// The audio proxy to use for testing. Additional connections can be made
    /// with `connect_to_audio_marker` if needed.
    audio_proxy: AudioProxy,
}

impl AudioTest {
    pub(crate) fn proxy(&self) -> &AudioProxy {
        &self.audio_proxy
    }

    pub(crate) async fn create_and_init(
        usages_to_report: &[AudioRenderUsage2],
    ) -> Result<Self, anyhow::Error> {
        // Setup the request channel used by the audio core mock. On startup
        // the service will generate 2 events per audio render usage, we'll
        // set a buffer large enough to hold them. Since the receiver's
        // capacity takes into account the number of senders we'll subtract one
        // from the buffer size for the sender we create here. The sender end
        // will be handed off to the core audio mock, this AudioTest instance
        // will retain the receiver end for verification of end-to-end message
        // flow.
        let buffer_size = (usages_to_report.len() * 2) - 1;
        let (audio_request_sender, request_receiver) =
            futures::channel::mpsc::channel::<AudioCoreRequest>(buffer_size);

        // Setup the test realm and connect an audio proxy to it.
        let realm = AudioTest::create_realm(audio_request_sender, usages_to_report).await?;
        let audio_proxy = AudioTest::connect_to_audio_marker_internal(&realm);

        let mut test_instance = Self { realm, request_receiver, audio_proxy };

        // Verify that audio core receives the initial volume settings on start.
        test_instance
            .wait_for_initial_audio_requests(&usages_to_report)
            .await
            .expect("initial audio requests received");

        Ok(test_instance)
    }

    /// Creates a test realm.
    ///
    /// `audio_core_request_sender` is used to report when requests are processed by the audio core
    /// mock.
    ///
    /// `usages_to_report` is a list of usages to report requests for. Any usages not in this list
    /// will not have `AudioCoreRequest` sent when processed.
    async fn create_realm(
        audio_core_request_sender: Sender<AudioCoreRequest>,
        usages_to_report: &[AudioRenderUsage2],
    ) -> Result<RealmInstance, Error> {
        let builder = RealmBuilder::new().await?;
        // Add setui_service as child of the realm builder.
        let setui_service =
            builder.add_child("setui_service", COMPONENT_URL, ChildOptions::new()).await?;
        let info = utils::SettingsRealmInfo {
            builder,
            settings: &setui_service,
            has_config_data: true,
            capabilities: vec![AudioMarker::PROTOCOL_NAME],
        };
        // Add basic Settings service realm information.
        utils::create_realm_basic(&info).await?;

        // Add mock audio core service.
        let mut streams = HashMap::<AudioRenderUsage2, (f32, bool)>::new();
        for stream in default_streams2() {
            let _ = streams.insert(
                stream.stream.expect("stream usage specified"),
                (
                    stream
                        .user_volume
                        .as_ref()
                        .expect("user volume specified")
                        .level
                        .expect("stream level specified"),
                    stream
                        .user_volume
                        .as_ref()
                        .expect("user volume specified")
                        .muted
                        .expect("stream level specified"),
                ),
            );
        }
        let usages_vec = usages_to_report.to_vec();
        let audio_streams = Arc::new(RwLock::new(streams));
        let audio_service = info
            .builder
            .add_local_child(
                "audio_service",
                move |handles: LocalComponentHandles| {
                    Box::pin(audio_core_service_mock(
                        handles,
                        audio_streams.clone(),
                        audio_core_request_sender.clone(),
                        usages_vec.clone(),
                    ))
                },
                ChildOptions::new().eager(),
            )
            .await?;

        // Route the mock audio core service through the parent realm to setui_service.
        info.builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol_by_name(AudioCoreMarker::PROTOCOL_NAME))
                    .from(&audio_service)
                    .to(Ref::parent())
                    .to(&setui_service),
            )
            .await?;

        // Provide LogSink to print out logs of the audio core component for debugging purposes.
        info.builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                    .from(Ref::parent())
                    .to(&audio_service),
            )
            .await?;

        let instance = info.builder.build().await?;
        Ok(instance)
    }

    fn connect_to_audio_marker_internal(instance: &RealmInstance) -> AudioProxy {
        return instance.root.connect_to_protocol_at_exposed_dir().expect("connecting to Audio");
    }

    pub(crate) fn connect_to_audio_marker(&mut self) -> AudioProxy {
        AudioTest::connect_to_audio_marker_internal(&self.realm)
    }

    /// Verifies that the next requests on the given receiver are equal to the
    /// provided requests, though not necessarily in order.
    ///
    /// We don't verify in order since the settings service can send two
    /// requests for one volume set, as well as two requests per usage type on
    /// start and there's no requirement for ordering.
    ///
    /// Note that this only verifies as many requests as are in the provided
    /// vector. If not enough requests are queued up, this will stall.
    pub(crate) async fn verify_audio_requests(
        &mut self,
        expected_requests: &[AudioCoreRequest],
    ) -> Result<(), anyhow::Error> {
        let mut received_requests = Vec::<AudioCoreRequest>::new();
        for _ in 0..expected_requests.len() {
            received_requests
                .push(self.request_receiver.next().await.expect("received audio core request"));
        }
        for request in expected_requests {
            if !received_requests.contains(request) {
                return Err(format_err!(
                    "Request {:?} not found in {:?}",
                    request,
                    received_requests
                ));
            }
        }
        Ok(())
    }

    /// Waits for the set of initial audio requests to be processed and clears
    /// them out of the receiver queue.
    async fn wait_for_initial_audio_requests(
        &mut self,
        audio_renderer_usages: &[AudioRenderUsage2],
    ) -> Result<(), anyhow::Error> {
        let expected_requests: Vec<_> = audio_renderer_usages
            .iter()
            .map(|&usage| {
                [
                    AudioCoreRequest::SetVolume(usage, DEFAULT_VOLUME_LEVEL),
                    AudioCoreRequest::SetMute(usage, DEFAULT_VOLUME_MUTED),
                ]
            })
            .flatten()
            .collect();
        self.verify_audio_requests(&expected_requests).await
    }

    /// Destroy realm instance after each test to avoid unexpected behavior.
    pub(crate) async fn clean_up(self) {
        let _ = self.realm.destroy().await;
    }
}
