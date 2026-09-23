// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::types::{AudioError, AudioStream};
use crate::utils::round_volume_level;
use fidl::endpoints::create_proxy;
use fidl_fuchsia_media::Usage2;
use fidl_fuchsia_media_audio::VolumeControlProxy;
use futures::TryStreamExt;
#[cfg(test)]
use futures::channel::mpsc::UnboundedSender;
use futures::channel::oneshot::Sender;
#[cfg(test)]
use settings_common::clock::mock as clock;
use settings_common::inspect::event::ExternalEventPublisher;
#[cfg(test)]
use settings_common::service_context::ExternalServiceEvent;
use settings_common::service_context::ExternalServiceProxy;
use settings_common::{call, trace, trace_guard};
use std::rc::Rc;
use {fuchsia_async as fasync, fuchsia_trace as ftrace};

#[cfg(test)]
const PUBLISHER_EVENT_NAME: &str = "volume_control_events";
const CONTROLLER_ERROR_DEPENDENCY: &str = "fuchsia.media.audio";
#[cfg(test)]
const UNKNOWN_INSPECT_STRING: &str = "unknown";

/// Closure definition for an action that can be triggered by ActionFuse.
pub(crate) type ExitAction = Rc<dyn Fn()>;

// Stores an AudioStream and a VolumeControl proxy bound to the AudioCore
// service for |stored_stream|'s stream type. |proxy| is set to None if it
// fails to bind to the AudioCore service. |early_exit_action| specifies a
// closure to be run if the StreamVolumeControl exits prematurely.
pub struct StreamVolumeControl {
    pub stored_stream: AudioStream,
    proxy: Option<VolumeControlProxy>,
    audio_service: ExternalServiceProxy<fidl_fuchsia_media::AudioCoreProxy, ExternalEventPublisher>,
    early_exit_action: Option<ExitAction>,
    #[cfg(test)]
    publisher: Option<UnboundedSender<ExternalServiceEvent>>,
    listen_exit_tx: Option<Sender<()>>,
}

impl Drop for StreamVolumeControl {
    fn drop(&mut self) {
        if let Some(exit_tx) = self.listen_exit_tx.take() {
            // Do not signal exit if receiver is already closed.
            if exit_tx.is_canceled() {
                return;
            }

            // Consider panic! is likely to be abort in the drop method, only log info for
            // unbounded_send failure.
            exit_tx.send(()).unwrap_or_else(|_| {
                log::warn!("StreamVolumeControl::drop, exit_tx failed to send exit signal")
            });
        }
    }
}

impl StreamVolumeControl {
    pub(crate) async fn create(
        id: ftrace::Id,
        audio_service: ExternalServiceProxy<
            fidl_fuchsia_media::AudioCoreProxy,
            ExternalEventPublisher,
        >,
        stream: AudioStream,
        early_exit_action: Option<ExitAction>,
        #[cfg(test)] publisher: Option<UnboundedSender<ExternalServiceEvent>>,
    ) -> Result<Self, AudioError> {
        // Stream input should be valid. Input comes from restore should be valid
        // and from set request has the validation.
        assert!(stream.has_valid_volume_level());

        trace!(id, c"StreamVolumeControl ctor");
        let mut control = StreamVolumeControl {
            stored_stream: stream,
            proxy: None,
            audio_service,
            listen_exit_tx: None,
            early_exit_action,
            #[cfg(test)]
            publisher,
        };

        control.bind_volume_control(id).await?;
        Ok(control)
    }

    pub(crate) async fn set_volume(
        &mut self,
        id: ftrace::Id,
        stream: AudioStream,
    ) -> Result<(), AudioError> {
        assert_eq!(self.stored_stream.stream_type, stream.stream_type);
        // Stream input should be valid. Input comes from restore should be valid
        // and from set request has the validation.
        assert!(stream.has_valid_volume_level());

        // Try to create and bind a new VolumeControl.
        if self.proxy.is_none() {
            self.bind_volume_control(id).await?;
        }

        // Round volume level from user input.
        let mut new_stream_value = stream;
        new_stream_value.user_volume_level = round_volume_level(stream.user_volume_level);

        let proxy = self.proxy.as_ref().expect("no volume control proxy");

        if (self.stored_stream.user_volume_level - new_stream_value.user_volume_level).abs()
            > f32::EPSILON
            && let Err(e) = proxy.set_volume(new_stream_value.user_volume_level)
        {
            self.stored_stream = new_stream_value;
            return Err(AudioError::ExternalFailure(
                CONTROLLER_ERROR_DEPENDENCY,
                "set volume".into(),
                format!("{e:?}"),
            ));
        }

        if self.stored_stream.user_volume_muted != new_stream_value.user_volume_muted
            && let Err(e) = proxy.set_mute(stream.user_volume_muted)
        {
            self.stored_stream = new_stream_value;
            return Err(AudioError::ExternalFailure(
                CONTROLLER_ERROR_DEPENDENCY,
                "set mute".into(),
                format!("{e:?}"),
            ));
        }

        self.stored_stream = new_stream_value;
        Ok(())
    }

    async fn bind_volume_control(&mut self, id: ftrace::Id) -> Result<(), AudioError> {
        trace!(id, c"bind volume control");
        if self.proxy.is_some() {
            return Ok(());
        }

        let (vol_control_proxy, server_end) = create_proxy();
        let stream_type = self.stored_stream.stream_type;
        let usage = Usage2::RenderUsage(stream_type.into());

        let guard = trace_guard!(id, c"bind usage volume control");
        if let Err(e) = call!(self.audio_service => bind_usage_volume_control2(&usage, server_end))
        {
            return Err(AudioError::ExternalFailure(
                CONTROLLER_ERROR_DEPENDENCY,
                format!("bind_usage_volume_control2 for audio_core {usage:?}").into(),
                format!("{e:?}"),
            ));
        }
        drop(guard);

        let guard = trace_guard!(id, c"set values");
        // Once the volume control is bound, apply the persisted audio settings to it.
        if let Err(e) = vol_control_proxy.set_volume(self.stored_stream.user_volume_level) {
            return Err(AudioError::ExternalFailure(
                CONTROLLER_ERROR_DEPENDENCY,
                format!("set_volume for vol_control {stream_type:?}").into(),
                format!("{e:?}"),
            ));
        }

        if let Err(e) = vol_control_proxy.set_mute(self.stored_stream.user_volume_muted) {
            return Err(AudioError::ExternalFailure(
                CONTROLLER_ERROR_DEPENDENCY,
                "set_mute for vol_control".into(),
                format!("{e:?}"),
            ));
        }
        drop(guard);

        if let Some(exit_tx) = self.listen_exit_tx.take() {
            // exit_rx needs this signal to end leftover spawn.
            exit_tx.send(()).expect(
                "StreamVolumeControl::bind_volume_control, listen_exit_tx failed to send exit \
                signal",
            );
        }

        trace!(id, c"setup listener");

        let (exit_tx, mut exit_rx) = futures::channel::oneshot::channel::<()>();
        let mut volume_events = vol_control_proxy.take_event_stream();
        let early_exit_action = self.early_exit_action.clone();
        fasync::Task::local({
            #[cfg(test)]
            let publisher = self.publisher.clone();
            async move {
                let id = ftrace::Id::new();
                trace!(id, c"bind volume handler");
                loop {
                    futures::select! {
                        _ = exit_rx => {
                            trace!(id, c"exit");
                            #[cfg(test)]
                            {
                                // Send UNKNOWN_INSPECT_STRING for request-related args because it
                                // can't be tied back to the event that caused the proxy to close.
                                if let Some(publisher) = publisher {
                                    let _ = publisher.unbounded_send(
                                        ExternalServiceEvent::Closed(
                                            PUBLISHER_EVENT_NAME,
                                            UNKNOWN_INSPECT_STRING.into(),
                                            UNKNOWN_INSPECT_STRING.into(),
                                            clock::inspect_format_now().into(),
                                        )
                                    );
                                }
                            }
                            return;
                        }
                        volume_event = volume_events.try_next() => {
                            trace!(id, c"volume_event");
                            if let Err(_) | Ok(None) = volume_event {
                                if let Some(action) = early_exit_action {
                                    (action)();
                                }
                                return;
                            }
                        }
                    }
                }
            }
        })
        .detach();

        self.listen_exit_tx = Some(exit_tx);
        self.proxy = Some(vol_control_proxy);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fakes::audio_core_service;
    use crate::types::{AudioInfo, AudioStreamType};
    use crate::{StreamVolumeControl, build_audio_default_settings, create_default_audio_stream};
    use fuchsia_inspect::component;
    use futures::StreamExt;
    use futures::channel::mpsc;
    use futures::lock::Mutex;
    use settings_common::inspect::config_logger::InspectConfigLogger;
    use settings_common::service_context::ServiceContext;
    use settings_test_common::fakes::service::ServiceRegistry;

    fn default_audio_info() -> AudioInfo {
        let config_logger =
            Rc::new(std::sync::Mutex::new(InspectConfigLogger::new(component::inspector().root())));
        let mut audio_configuration = build_audio_default_settings(config_logger);
        audio_configuration
            .load_default_value()
            .expect("config should exist and parse for test")
            .unwrap()
    }

    // Returns a registry populated with the AudioCore service.
    async fn create_service() -> Rc<Mutex<ServiceRegistry>> {
        let service_registry = ServiceRegistry::create();
        let audio_core_service_handle = audio_core_service::Builder::new(default_audio_info())
            .set_suppress_client_errors(true)
            .build();
        service_registry.lock().await.register_service(audio_core_service_handle.clone());
        service_registry
    }

    // Tests that the volume event stream thread exits when the StreamVolumeControl is deleted.
    #[fuchsia::test(allow_stalls = false)]
    async fn test_drop_thread() {
        let service_context =
            ServiceContext::new(Some(ServiceRegistry::serve(create_service().await)));
        let (event_tx, _) = mpsc::unbounded();
        let external_publisher = ExternalEventPublisher::new(event_tx);

        let audio_proxy = service_context
            .connect_with_publisher::<fidl_fuchsia_media::AudioCoreMarker, _>(external_publisher)
            .await
            .expect("service should be present");

        let (event_tx, mut event_rx) = mpsc::unbounded();
        let _ = StreamVolumeControl::create(
            0.into(),
            audio_proxy,
            create_default_audio_stream(AudioStreamType::Media),
            None,
            Some(event_tx),
        )
        .await;
        let req = "unknown";
        let req_timestamp = "unknown";
        let resp_timestamp = clock::inspect_format_now();

        assert_eq!(
            event_rx.next().await.expect("First message should have been the closed event"),
            ExternalServiceEvent::Closed(
                "volume_control_events",
                req.into(),
                req_timestamp.into(),
                resp_timestamp.into(),
            )
        );
    }

    // Ensures that the StreamVolumeControl properly fires the provided early exit
    // closure when the underlying AudioCoreService closes unexpectedly.
    #[fuchsia::test(allow_stalls = false)]
    async fn test_detect_early_exit() {
        let service_registry = ServiceRegistry::create();
        let audio_core_service_handle = audio_core_service::Builder::new(default_audio_info())
            .set_suppress_client_errors(true)
            .build();
        service_registry.lock().await.register_service(audio_core_service_handle.clone());

        let service_context = ServiceContext::new(Some(ServiceRegistry::serve(service_registry)));
        let (event_tx, _) = mpsc::unbounded();
        let external_publisher = ExternalEventPublisher::new(event_tx);

        let audio_proxy = service_context
            .connect_with_publisher::<fidl_fuchsia_media::AudioCoreMarker, _>(external_publisher)
            .await
            .expect("proxy should be present");
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();

        // Create StreamVolumeControl, specifying firing an event as the early exit
        // action. Note that we must store the returned value or else the normal
        // drop behavior will clean up it before the AudioCoreService's exit can
        // be detected.
        let _stream_volume_control = StreamVolumeControl::create(
            0.into(),
            audio_proxy,
            create_default_audio_stream(AudioStreamType::Media),
            Some(Rc::new(move || {
                tx.unbounded_send(()).unwrap();
            })),
            None,
        )
        .await
        .expect("should successfully build");

        // Trigger AudioCoreService exit.
        audio_core_service_handle.lock().await.exit();

        // Check to make sure early exit event was received.
        assert!(matches!(rx.next().await, Some(..)));
    }
}
