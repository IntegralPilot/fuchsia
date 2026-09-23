// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::types::AudioInfo;
use anyhow::{Error, format_err};
use fidl::endpoints::ServerEnd;
use fidl::prelude::*;
use fidl_fuchsia_media::{AudioRenderUsage2, Usage2};
use fuchsia_async as fasync;
use fuchsia_sync::RwLock;
use futures::channel::mpsc::UnboundedSender;
use futures::channel::oneshot;
use futures::lock::Mutex;
use futures::{FutureExt, TryStreamExt};
use settings_test_common::fakes::service::Service;
use std::collections::HashMap;
use std::rc::Rc;

pub(crate) struct Builder {
    suppress_client_errors: bool,
    default_settings: AudioInfo,
}

impl Builder {
    pub(crate) fn new(default_settings: AudioInfo) -> Self {
        Self { suppress_client_errors: false, default_settings }
    }

    /// Sets whether errors originating from communicating with the client
    /// should be checked. If not suppressed, errors encountered will be fatal
    /// for the execution.
    pub(crate) fn set_suppress_client_errors(mut self, suppress: bool) -> Self {
        self.suppress_client_errors = suppress;
        self
    }

    pub(crate) fn build(self) -> Rc<Mutex<AudioCoreService>> {
        Rc::new(Mutex::new(AudioCoreService::new(
            self.suppress_client_errors,
            self.default_settings,
        )))
    }
}
/// An implementation of audio core service that captures the set gains on
/// usages.
pub(crate) struct AudioCoreService {
    suppress_client_errors: bool,
    audio_streams: Rc<RwLock<HashMap<AudioRenderUsage2, (f32, bool)>>>,
    event_tx: Option<UnboundedSender<()>>,
    exit_tx: Option<oneshot::Sender<()>>,
}

impl AudioCoreService {
    pub(crate) fn new(suppress_client_errors: bool, default_settings: AudioInfo) -> Self {
        let mut streams = HashMap::new();
        for stream in default_settings.streams.iter() {
            let _ = streams.insert(
                AudioRenderUsage2::from(stream.stream_type),
                (stream.user_volume_level, stream.user_volume_muted),
            );
        }
        Self {
            audio_streams: Rc::new(RwLock::new(streams)),
            suppress_client_errors,
            event_tx: None,
            exit_tx: None,
        }
    }

    pub(crate) fn set_event_tx(&mut self, event_tx: UnboundedSender<()>) {
        self.event_tx = Some(event_tx);
    }

    pub(crate) fn get_level_and_mute(&self, usage: AudioRenderUsage2) -> Option<(f32, bool)> {
        get_level_and_mute(usage, &self.audio_streams)
    }

    /// Causes the AudioCoreService to exit its request stream processing loop.
    /// Has no effect if the service is not currently processing requests.
    pub(crate) fn exit(&mut self) {
        if let Some(tx) = self.exit_tx.take() {
            let _ = tx.send(());
        }
    }
}

impl Service for AudioCoreService {
    fn can_handle_service(&self, service_name: &str) -> bool {
        service_name == fidl_fuchsia_media::AudioCoreMarker::PROTOCOL_NAME
    }

    fn process_stream(&mut self, service_name: &str, channel: zx::Channel) -> Result<(), Error> {
        if !self.can_handle_service(service_name) {
            return Err(format_err!("unsupported"));
        }

        let mut manager_stream =
            ServerEnd::<fidl_fuchsia_media::AudioCoreMarker>::new(channel).into_stream();

        let (tx, rx) = oneshot::channel::<()>();
        self.exit_tx = Some(tx);

        let streams_clone = self.audio_streams.clone();
        let suppress_client_errors = self.suppress_client_errors;
        let event_tx = self.event_tx.clone();

        fasync::Task::local(async move {
            let fused_exit = rx.fuse();
            futures::pin_mut!(fused_exit);

            loop {
                futures::select! {
                    _ = fused_exit => {
                        return;
                    }
                    req = manager_stream.try_next() => {
                        let request = req.unwrap();

                        if request.is_none() {
                            return;
                        }
                        if let fidl_fuchsia_media::AudioCoreRequest::BindUsageVolumeControl2 {
                            usage: Usage2::RenderUsage(render_usage),
                            volume_control,
                            control_handle: _,
                        } = request.expect("request should be present") {
                            process_volume_control_stream(
                                volume_control,
                                render_usage,
                                streams_clone.clone(),
                                suppress_client_errors,
                                event_tx.clone(),
                            );
                        }
                    }
                }
            }
        })
        .detach();

        Ok(())
    }
}

fn get_level_and_mute(
    usage: AudioRenderUsage2,
    streams: &RwLock<HashMap<AudioRenderUsage2, (f32, bool)>>,
) -> Option<(f32, bool)> {
    if let Some((level, muted)) = (*streams.read()).get(&usage) {
        return Some((*level, *muted));
    }
    None
}

fn process_volume_control_stream(
    volume_control: ServerEnd<fidl_fuchsia_media_audio::VolumeControlMarker>,
    render_usage: AudioRenderUsage2,
    streams: Rc<RwLock<HashMap<AudioRenderUsage2, (f32, bool)>>>,
    suppress_client_errors: bool,
    event_tx: Option<UnboundedSender<()>>,
) {
    let mut stream = volume_control.into_stream();
    fasync::Task::local(async move {
        while let Some(req) = stream.try_next().await.unwrap() {
            #[allow(unreachable_patterns)]
            match req {
                fidl_fuchsia_media_audio::VolumeControlRequest::SetVolume {
                    volume,
                    control_handle,
                } => {
                    let (_level, muted) =
                        get_level_and_mute(render_usage, &streams).expect("stream in map");
                    let _ = (*streams.write()).insert(render_usage, (volume, muted));

                    let on_volume_mute_changed_result =
                        control_handle.send_on_volume_mute_changed(volume, muted);

                    if !suppress_client_errors {
                        on_volume_mute_changed_result.expect("on volume mute changed");
                    }
                }
                fidl_fuchsia_media_audio::VolumeControlRequest::SetMute {
                    mute,
                    control_handle,
                } => {
                    let (level, _muted) =
                        get_level_and_mute(render_usage, &streams).expect("stream in map");
                    let _ = (*streams.write()).insert(render_usage, (level, mute));

                    let on_volume_mute_changed_result =
                        control_handle.send_on_volume_mute_changed(level, mute);

                    if !suppress_client_errors {
                        on_volume_mute_changed_result.expect("on volume mute changed");
                    }
                }
                _ => {}
            }

            if let Some(tx) = event_tx.as_ref() {
                let _ = tx.unbounded_send(());
            }
        }
    })
    .detach();
}
