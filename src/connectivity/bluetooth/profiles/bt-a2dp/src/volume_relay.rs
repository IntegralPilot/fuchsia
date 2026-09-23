// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error, format_err};
use async_utils::hanging_get::client::HangingGetStream;
use fidl::endpoints;
use fidl_fuchsia_bluetooth_avrcp as avrcp;
use fidl_fuchsia_media as media;
use fidl_fuchsia_settings as settings;
use fuchsia_async::{self as fasync, DurationExt, Timer};
use futures::channel::oneshot::Sender;
use futures::future::{Fuse, FusedFuture};
use futures::{Future, FutureExt, StreamExt, select};
use log::{info, trace, warn};
use std::fmt::Debug;
use std::pin::pin;

/// Represents the data payload of a set volume request attempt.
/// All volume values are AVRCP volume values with range [0, 127].
#[derive(Clone, Copy, Debug)]
struct SetVolumeRequestData {
    /// System volume value before the request was made.
    prev_system_vol: u8,
    /// Adjusted volume value we are trying to set (may include nudging).
    adj_requested_vol: u8,
    /// Original volume value requested by the peer.
    orig_requested_vol: u8,
}

impl SetVolumeRequestData {
    fn new(curr_system_vol: u8, orig_requested_vol: u8, adj_requested_vol: u8) -> Self {
        Self { prev_system_vol: curr_system_vol, adj_requested_vol, orig_requested_vol }
    }

    fn is_volume_changed(&self, new_volume: u8) -> bool {
        self.prev_system_vol != new_volume
    }

    /// Calculates the adjusted requested volume if it matches the previously requested volume
    /// of this failed request.
    ///
    /// This is a workaround for quantization/scaling deadlocks. If a request
    /// fails to change system volume, nudging it by one step breaks the deadlock.
    /// Only adjusts if the new request matches the volume from the previous request.
    fn calculate_adjusted_volume(&self, curr_system_vol: u8, new_requested_vol: u8) -> u8 {
        const AVRCP_VOLUME_STEP_SIZE: u8 = 1;
        const MAX_AVRCP_VOLUME: u8 = 127;

        if new_requested_vol != self.orig_requested_vol {
            trace!(
                "New SetVolume request {new_requested_vol} differs from previously requested volume {}, not adjusting",
                self.orig_requested_vol
            );
            return new_requested_vol;
        }

        let is_increment = new_requested_vol > curr_system_vol;

        // Nudge based on the previously adjusted volume so repeated retries continue to nudge the volume.
        match (is_increment, self.adj_requested_vol) {
            (true, adj)
                if adj
                    .checked_add(AVRCP_VOLUME_STEP_SIZE)
                    .is_some_and(|v| v <= MAX_AVRCP_VOLUME) =>
            {
                let adjusted = adj + AVRCP_VOLUME_STEP_SIZE;
                trace!("Requested volume adjusted from {new_requested_vol} to {adjusted}");
                adjusted
            }
            (false, adj) if adj.checked_sub(AVRCP_VOLUME_STEP_SIZE).is_some() => {
                let adjusted = adj - AVRCP_VOLUME_STEP_SIZE;
                trace!("Requested volume adjusted from {new_requested_vol} to {adjusted}");
                adjusted
            }
            _ => {
                trace!(
                    "Requested volume same as failed request but cannot be adjusted further ({new_requested_vol})"
                );
                new_requested_vol
            }
        }
    }
}

/// An active, pending SetVolume request (guaranteed to have a responder).
struct PendingSetVolume {
    data: SetVolumeRequestData,
    responder: fidl_fuchsia_bluetooth_avrcp::AbsoluteVolumeHandlerSetVolumeResponder,
}

impl PendingSetVolume {
    fn new(
        data: SetVolumeRequestData,
        responder: fidl_fuchsia_bluetooth_avrcp::AbsoluteVolumeHandlerSetVolumeResponder,
    ) -> Self {
        Self { data, responder }
    }

    fn send(self, new_volume: u8) {
        if let Err(e) = self.responder.send(new_volume) {
            warn!("Could not send SetVolume response: {:?}", e);
        }
    }
}

pub(crate) struct VolumeRelay {
    /// A sender that when sent will cause the relay task to stop. None if the task is not running.
    _stop: Option<Sender<()>>,
}

struct AvrcpVolume(u8);

impl AvrcpVolume {
    /// Convert from a settings volume between 0.0 and 1.0 to a volume that can be sent
    /// through AVRCP (0 to 127 as per the spec)
    fn from_media_volume(value: settings::AudioSettings2) -> Result<Self, anyhow::Error> {
        let streams =
            value.streams.ok_or_else(|| format_err!("No streams in the AudioSettings2"))?;

        // Find the media stream volume
        let volume =
            match streams.iter().find(|&s| s.stream == Some(media::AudioRenderUsage2::Media)) {
                None => Err(format_err!("Couldn't find Media stream in settings")),
                Some(settings::AudioStreamSettings2 { user_volume: None, .. }) => {
                    Err(format_err!("Volume not included in Media stream settings"))
                }
                Some(settings::AudioStreamSettings2 { user_volume: Some(vol), .. }) => Ok(vol),
            };
        let level = match volume? {
            settings::Volume { muted: Some(true), .. } => 0.0,
            settings::Volume { level: None, .. } => 0.0,
            settings::Volume { level: Some(vol), .. } => *vol,
        };

        Ok(AvrcpVolume((level * 127.0) as u8))
    }

    /// Get an AudioSettings2 struct that can be sent to the Settings service to set the volume
    /// to the same level as this.  Converts from native AVRCP (0-127) to settings 0.0 - 1.0
    /// ranges.
    fn as_audio_settings(&self, stream: media::AudioRenderUsage2) -> settings::AudioSettings2 {
        let settings = settings::AudioStreamSettings2 {
            stream: Some(stream),
            source: Some(settings::AudioStreamSettingSource::User),
            user_volume: Some(settings::Volume {
                level: Some(self.0 as f32 / 127.0),
                muted: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        };
        settings::AudioSettings2 { streams: Some(vec![settings]), ..Default::default() }
    }
}

/// How long we will wait for the system to report a new volume after requesting a new volume.
/// This is chosen as 100 milliseconds as half of the required response time in the AVRCP Spec,
/// Section 6.2
const SETVOLUME_TIMEOUT: zx::MonotonicDuration = zx::MonotonicDuration::from_millis(100);

impl VolumeRelay {
    /// Start a relay between AVRCP and Settings.Audio.
    /// Media Volume is reported to AVRCP for Absolute Volume Controllers, and changes from AVRCP
    /// are propagated to the system Media volume.
    /// This starts the relay.  The relay can be stopped by dropping it.
    pub(crate) fn start() -> Result<Self, Error> {
        let avrcp_svc =
            fuchsia_component::client::connect_to_protocol::<avrcp::PeerManagerMarker>()
                .context("Failed to connect to Bluetooth AVRCP interface")?;
        let audio_settings_svc =
            fuchsia_component::client::connect_to_protocol::<settings::AudioMarker>()
                .context("Failed to connect to Audio settings interface")?;

        let (sender, receiver) = futures::channel::oneshot::channel();

        spawn_err("Volume", Self::volume_relay(avrcp_svc, audio_settings_svc, receiver.fuse()));

        Ok(Self { _stop: Some(sender) })
    }

    async fn volume_relay(
        mut avrcp: avrcp::PeerManagerProxy,
        audio: settings::AudioProxy,
        mut stop_signal: impl FusedFuture + Unpin,
    ) -> Result<(), Error> {
        let mut volume_requests =
            connect_avrcp_volume(&mut avrcp).await.context("connecting avrcp volume")?;

        let audio_proxy_clone = audio.clone();
        let mut audio_watch_stream =
            HangingGetStream::new(audio_proxy_clone, settings::AudioProxy::watch2);

        // Wait for the first update from the settings app.
        let mut current_volume = match audio_watch_stream.next().await {
            None => return Err(format_err!("Volume watch response stream ended")),
            Some(Err(e)) => return Err(format_err!("FIDL error polling audio watch2: {:?}", e)),
            Some(Ok(settings)) => match AvrcpVolume::from_media_volume(settings) {
                Err(e) => return Err(format_err!("Can't get initial volume: {:?}", e)),
                Ok(vol) => vol.0,
            },
        };
        trace!("Initial system media volume level is {:?} in AVRCP", current_volume);
        let mut staged_volume = Some(current_volume);

        let mut last_onchanged = None;
        // TODO(https://fxbug.dev/42131477): Change this to be a single responder when AVRCP correctly manages the
        // lifetime of volume changed subscriptions.
        let mut hanging_onchanged = Vec::new();
        let mut hanging_setvolumes: Vec<PendingSetVolume> = Vec::new();
        // Keeps track of most recent failed set volume request to enable
        // nudging adjustments on valid retries.
        let mut failed_setvolume: Option<SetVolumeRequestData> = None;

        let setvolume_timeout = Fuse::terminated();
        let mut setvolume_timeout = pin!(setvolume_timeout);

        loop {
            let mut sys_volume_watch_fut = audio_watch_stream.next();
            let mut avrcp_request_fut = volume_requests.next();

            select! {
                _ = stop_signal => {
                    break Ok(());
                },
                avrcp_request = avrcp_request_fut => {
                    let request = match avrcp_request {
                        None => return Err(format_err!("AVRCP Volume Handler Channel Closed")),
                        Some(Err(e)) => return Err(format_err!("Volume Handler Request Error: {:?}", e)),
                        Some(Ok(req)) => req,
                    };
                    match request {
                        avrcp::AbsoluteVolumeHandlerRequest::SetVolume { mut requested_volume, responder } => {
                            if requested_volume == current_volume {
                                warn!("Requested AVRCP volume same as current volume: {requested_volume}");
                                let _ = responder.send(current_volume);
                                continue;
                            }

                            let original_requested = requested_volume;
                            if let Some(failed_req) = &failed_setvolume {
                                requested_volume = failed_req.calculate_adjusted_volume(current_volume, original_requested);
                            }

                            let settings = AvrcpVolume(requested_volume).as_audio_settings(media::AudioRenderUsage2::Media);
                            trace!("AVRCP Setting system volume to {} -> {:?}", requested_volume, settings);
                            if let Err(e) = audio.set2(&settings).await {
                                warn!("Couldn't set media volume: {:?}", e);
                                let _ = responder.send(current_volume);
                                continue;
                            }
                            let _ = failed_setvolume.take();
                            let data = SetVolumeRequestData::new(current_volume, original_requested, requested_volume);
                            hanging_setvolumes.push(PendingSetVolume::new(data, responder));
                            if setvolume_timeout.is_terminated() {
                                setvolume_timeout.set(Timer::new(SETVOLUME_TIMEOUT.after_now()).fuse());
                            }
                        },
                        avrcp::AbsoluteVolumeHandlerRequest::OnVolumeChanged { responder } => {
                            hanging_onchanged.push(responder);
                        },
                        avrcp::AbsoluteVolumeHandlerRequest::GetCurrentVolume { responder } => {
                            let _ = responder.send(current_volume);
                            continue;
                        }
                    }
                },
                _ = setvolume_timeout => {
                    hanging_setvolumes.drain(..).for_each(|req| {
                        // TODO(b/250265882): convert the log back to trace once issue is resolved.
                        info!("SetVolume request timed out: {:?}. Current volume: {current_volume}", req.data);
                        if failed_setvolume.is_none() {
                            failed_setvolume = Some(req.data);
                        }
                        req.send(current_volume);
                    });
                },
                watch_response = sys_volume_watch_fut => {
                    let settings = match watch_response {
                        None => return Err(format_err!("Volume watch response stream ended")),
                        Some(Err(e)) => return Err(format_err!("FIDL error from watch: {:?}", e)),
                        Some(Ok(settings)) => settings,
                    };

                    current_volume = match AvrcpVolume::from_media_volume(settings) {
                        Err(e) => {
                            warn!("Volume Relay can't get volume: {:?}", e);
                            continue;
                        },
                        Ok(vol) => vol.0,
                    };

                    trace!("System media volume level now at {:?} in AVRCP", current_volume);
                    if hanging_setvolumes.len() > 0 {
                        hanging_setvolumes = hanging_setvolumes.into_iter().filter_map(|req| {
                            if req.data.is_volume_changed(current_volume) {
                                req.send(current_volume);
                                return None;
                            }
                            Some(req)
                        }).collect();
                        // When the change is the result of a setvolume command, the onchanged
                        // hanging is _not_ updated.
                        last_onchanged = Some(current_volume);
                        continue;
                    }
                    staged_volume = Some(current_volume);
                },
            }

            if !hanging_onchanged.is_empty() && staged_volume.is_some() {
                let next_volume = staged_volume.take().unwrap();
                if Some(next_volume) == last_onchanged {
                    trace!("Not reporting unchanged volume {} to AVRCP", next_volume);
                    continue;
                }

                for responder in hanging_onchanged.drain(..) {
                    let _ = responder.send(next_volume);
                }
                trace!("Reporting changed system volume {} to AVRCP", next_volume);
                last_onchanged = Some(next_volume);
            }
        }
    }
}

fn spawn_err<F, E>(label: &'static str, future: F)
where
    F: Future<Output = Result<(), E>> + Send + 'static,
    E: Debug,
{
    fasync::Task::spawn(async move {
        if let Some(e) = future.await.err() {
            info!("{} Completed with Error: {:?}", label, e);
        }
    })
    .detach();
}

async fn connect_avrcp_volume(
    avrcp: &mut avrcp::PeerManagerProxy,
) -> Result<avrcp::AbsoluteVolumeHandlerRequestStream, Error> {
    let (client, request_stream) = endpoints::create_request_stream();

    if let Err(e) = avrcp.set_absolute_volume_handler(client).await? {
        info!("failed to set absolute volume handler");
        return Err(format_err!("Failed setting absolute volume handler: {}", e));
    }

    Ok(request_stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_matches::assert_matches;
    use async_utils::PollExt;

    use std::pin::Pin;

    const INITIAL_MEDIA_VOLUME: f32 = 0.8;
    const INITIAL_AVRCP_VOLUME: u8 = 101;

    fn setup_avrcp_proxy() -> (avrcp::PeerManagerProxy, avrcp::PeerManagerRequestStream) {
        endpoints::create_proxy_and_stream::<avrcp::PeerManagerMarker>()
    }

    fn setup_settings_proxy() -> (settings::AudioProxy, settings::AudioRequestStream) {
        endpoints::create_proxy_and_stream::<settings::AudioMarker>()
    }

    /// Builds all of the Proxies and request streams involved with setting up a Volume Relay
    /// test.
    fn setup_volume_relay() -> (
        settings::AudioRequestStream,
        avrcp::PeerManagerRequestStream,
        Sender<()>,
        impl Future<Output = Result<(), Error>>,
    ) {
        let (settings_proxy, settings_requests) = setup_settings_proxy();
        let (avrcp_proxy, avrcp_requests) = setup_avrcp_proxy();

        let (stop_sender, receiver) = futures::channel::oneshot::channel();

        let relay_fut = VolumeRelay::volume_relay(avrcp_proxy, settings_proxy, receiver.fuse());
        (settings_requests, avrcp_requests, stop_sender, relay_fut)
    }

    /// Expects a Watch2() call to the `audio_request_stream`.  Returns the handler to respond to
    /// the watch2 call, or panics if that doesn't happen.
    #[track_caller]
    fn expect_audio_watch2(
        exec: &mut fasync::TestExecutor,
        audio_request_stream: &mut settings::AudioRequestStream,
    ) -> settings::AudioWatch2Responder {
        let watch_request_fut = audio_request_stream.select_next_some();
        let mut watch_request_fut = pin!(watch_request_fut);

        match exec.run_until_stalled(&mut watch_request_fut).expect("should be ready") {
            Ok(settings::AudioRequest::Watch2 { responder }) => responder,
            x => panic!("Expected an Audio Watch2 Request, got {:?}", x),
        }
    }

    #[track_caller]
    fn respond_to_audio_watch2(responder: settings::AudioWatch2Responder, level: f32) {
        responder
            .send(&settings::AudioSettings2 {
                streams: Some(vec![settings::AudioStreamSettings2 {
                    stream: Some(media::AudioRenderUsage2::Media),
                    user_volume: Some(settings::Volume {
                        level: Some(level),
                        ..Default::default()
                    }),
                    ..Default::default()
                }]),
                ..Default::default()
            })
            .expect("watch responder to send");
    }

    /// Confirms the setup of the Volume relay, which includes the registration of a volume
    /// handler proxy with the AVRCP client, and the initial volume setting request to the Media
    /// system.
    fn finish_relay_setup<T: Future>(
        mut relay_fut: &mut Pin<&mut T>,
        mut exec: &mut fasync::TestExecutor,
        mut avrcp_request_stream: avrcp::PeerManagerRequestStream,
        audio_request_stream: &mut settings::AudioRequestStream,
    ) -> (avrcp::AbsoluteVolumeHandlerProxy, settings::AudioWatch2Responder)
    where
        <T as Future>::Output: Debug,
    {
        // Expect registration of a AbsoluteVolumeHandler
        let request_fut = avrcp_request_stream.select_next_some();
        let mut request_fut = pin!(request_fut);

        let handler = match exec.run_until_stalled(&mut request_fut).expect("should be ready") {
            Ok(avrcp::PeerManagerRequest::SetAbsoluteVolumeHandler { handler, responder }) => {
                responder.send(Ok(())).expect("response to handler set");
                handler
            }
            x => panic!("Expected SetAbsoluteVolumeHandler, got: {:?}", x),
        };

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        let audio_watch_responder = expect_audio_watch2(&mut exec, audio_request_stream);
        respond_to_audio_watch2(audio_watch_responder, INITIAL_MEDIA_VOLUME);

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");
        let audio_watch_responder = expect_audio_watch2(&mut exec, audio_request_stream);

        (handler.into_proxy(), audio_watch_responder)
    }

    /// Test that the relay sets up the connection to AVRCP and Sessions and stops on the stop
    /// signal.
    #[fuchsia::test]
    fn relay_setup() {
        let mut exec = fasync::TestExecutor::new();
        let (mut settings_requests, avrcp_requests, stop_sender, relay_fut) = setup_volume_relay();

        let mut relay_fut = pin!(relay_fut);

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        let (volume_client, _watch_responder) =
            finish_relay_setup(&mut relay_fut, &mut exec, avrcp_requests, &mut settings_requests);

        // Sending a stop should drop all the things and the future should complete.
        stop_sender.send(()).expect("should be able to send a stop");

        let _ = exec.run_until_stalled(&mut relay_fut).expect("should be ready");

        assert!(
            exec.run_until_stalled(&mut settings_requests.next())
                .expect("should be ready")
                .is_none()
        );

        let mut current_volume_fut = volume_client.get_current_volume();
        assert!(exec.run_until_stalled(&mut current_volume_fut).expect("should be ready").is_err());
    }

    /// Test that the relay calls the set volume command correctly and responds within an
    /// appropriate amount of time.
    #[fuchsia::test]
    fn set_volume_command() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let (mut settings_requests, avrcp_requests, _stop_sender, relay_fut) = setup_volume_relay();

        let mut relay_fut = pin!(relay_fut);

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        let (volume_client, watch_responder) =
            finish_relay_setup(&mut relay_fut, &mut exec, avrcp_requests, &mut settings_requests);

        // Set volume request with the same volume value as the initial volume.
        let volume_set_fut = volume_client.set_volume(INITIAL_AVRCP_VOLUME);
        let mut volume_set_fut = pin!(volume_set_fut);

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        let request_fut = settings_requests.select_next_some();
        let mut request_fut = pin!(request_fut);

        // Response is returned immediately with the initial value.
        exec.run_until_stalled(&mut request_fut)
            .expect_pending("audio settings change request should not have been sent");
        assert_matches!(
            exec.run_until_stalled(&mut volume_set_fut).expect("should be ready"),
            Ok(INITIAL_AVRCP_VOLUME)
        );

        // Volume decrease request to 90.
        let volume_set_fut = volume_client.set_volume(90);
        let mut volume_set_fut = pin!(volume_set_fut);

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        exec.run_until_stalled(&mut volume_set_fut).expect_pending("should be pending");

        match exec.run_until_stalled(&mut request_fut).expect("should be ready") {
            Ok(settings::AudioRequest::Set2 { settings, responder }) => {
                assert!(
                    settings.streams.as_ref().unwrap()[0]
                        .user_volume
                        .as_ref()
                        .unwrap()
                        .level
                        .unwrap()
                        < INITIAL_MEDIA_VOLUME
                );
                let _ = responder.send(Ok(())).unwrap();
            }
            x => panic!("Expected Ready audio set request and got: {:?}", x),
        };

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        // When a new volume happens as a result, it's returned.
        respond_to_audio_watch2(watch_responder, 0.7);

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        assert_matches!(
            exec.run_until_stalled(&mut volume_set_fut).expect("should be ready"),
            Ok(88 /* 0.7 audio settings volume to AVRCP volume */)
        );
    }

    #[fuchsia::test]
    fn set_volume_command_retry() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let (mut settings_requests, avrcp_requests, _stop_sender, relay_fut) = setup_volume_relay();

        let mut relay_fut = pin!(relay_fut);

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        let (volume_client, watch_responder) =
            finish_relay_setup(&mut relay_fut, &mut exec, avrcp_requests, &mut settings_requests);

        // Volume increase request to 103.
        let volume_set_fut = volume_client.set_volume(103);
        let mut volume_set_fut = pin!(volume_set_fut);

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        let request_fut = settings_requests.select_next_some();
        let mut request_fut = pin!(request_fut);

        exec.run_until_stalled(&mut volume_set_fut).expect_pending("should be pending");

        let mut requested_audio_settings_vol: f32;
        match exec.run_until_stalled(&mut request_fut).expect("should be ready") {
            Ok(settings::AudioRequest::Set2 { settings, responder }) => {
                requested_audio_settings_vol = settings.streams.as_ref().unwrap()[0]
                    .user_volume
                    .as_ref()
                    .unwrap()
                    .level
                    .unwrap();
                assert_eq!(1, settings.streams.expect("a stream was set").len());
                let _ = responder.send(Ok(())).unwrap();
            }
            x => panic!("Expected Ready audio set request and got: {:?}", x),
        };

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        // Mimic no volume change by waiting out the maximum time we will wait for a new volume.
        exec.set_fake_time(zx::MonotonicDuration::from_millis(105).after_now());
        let _ = exec.wake_expired_timers();

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        // Because no change was sent from Media, the initial value from media is sent.
        assert_matches!(
            exec.run_until_stalled(&mut volume_set_fut).expect("should be ready"),
            Ok(INITIAL_AVRCP_VOLUME)
        );

        // Second set volume request to 103.
        let volume_set_fut = volume_client.set_volume(103);
        let mut volume_set_fut = pin!(volume_set_fut);

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        exec.run_until_stalled(&mut volume_set_fut).expect_pending("should be pending");

        let request_fut = settings_requests.select_next_some();
        let mut request_fut = pin!(request_fut);

        match exec.run_until_stalled(&mut request_fut).expect("should be ready") {
            Ok(settings::AudioRequest::Set2 { settings, responder }) => {
                // We should have adjusted the volume value to be larger for the second request.
                let new_requested = settings.streams.as_ref().unwrap()[0]
                    .user_volume
                    .as_ref()
                    .unwrap()
                    .level
                    .unwrap();
                assert!(new_requested > requested_audio_settings_vol);
                let _ = responder.send(Ok(())).unwrap();
                requested_audio_settings_vol = new_requested;
            }
            x => panic!("Expected Ready audio set request and got: {:?}", x),
        };

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        // Mimic no volume change by waiting out the maximum time we will wait for a new volume.
        exec.set_fake_time(zx::MonotonicDuration::from_millis(105).after_now());
        let _ = exec.wake_expired_timers();

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        // Because no change was sent from Media, the initial value from media is sent.
        assert_matches!(
            exec.run_until_stalled(&mut volume_set_fut).expect("should be ready"),
            Ok(INITIAL_AVRCP_VOLUME)
        );

        // Third set volume request to 103.
        let volume_set_fut = volume_client.set_volume(103);
        let mut volume_set_fut = pin!(volume_set_fut);

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        exec.run_until_stalled(&mut volume_set_fut).expect_pending("should be pending");

        let request_fut = settings_requests.select_next_some();
        let mut request_fut = pin!(request_fut);

        match exec.run_until_stalled(&mut request_fut).expect("should be ready") {
            Ok(settings::AudioRequest::Set2 { settings, responder }) => {
                // We should have adjusted the volume value again.
                let new_requested = settings.streams.as_ref().unwrap()[0]
                    .user_volume
                    .as_ref()
                    .unwrap()
                    .level
                    .unwrap();
                assert!(new_requested > requested_audio_settings_vol);
                let _ = responder.send(Ok(())).unwrap();
            }
            x => panic!("Expected Ready audio set request and got: {:?}", x),
        };

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        // When a new volume happens as a result, it's returned.
        respond_to_audio_watch2(watch_responder, 0.82);

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        assert_matches!(
            exec.run_until_stalled(&mut volume_set_fut).expect("should be ready"),
            Ok(104 /* 0.82 audio settings volume as AVRCP volume */)
        );
    }

    #[fuchsia::test]
    fn failed_request_does_not_affect_different_requests() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let (mut settings_requests, avrcp_requests, _stop_sender, relay_fut) = setup_volume_relay();

        let mut relay_fut = pin!(relay_fut);

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        let (volume_client, _watch_responder) =
            finish_relay_setup(&mut relay_fut, &mut exec, avrcp_requests, &mut settings_requests);

        // 1. Trigger a timeout with a high volume request (127).
        let volume_set_fut = volume_client.set_volume(127);
        let mut volume_set_fut = pin!(volume_set_fut);

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        let request_fut = settings_requests.select_next_some();
        let mut request_fut = pin!(request_fut);

        exec.run_until_stalled(&mut volume_set_fut).expect_pending("should be pending");

        // Consume/respond to volume set request, but don't confirm actual system
        // volume change to cause a timeout.
        match exec.run_until_stalled(&mut request_fut).expect("should be ready") {
            Ok(settings::AudioRequest::Set2 { responder, .. }) => {
                let _ = responder.send(Ok(())).unwrap();
            }
            x => panic!("Expected Ready audio set request and got: {:?}", x),
        };

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        // Advance time to trigger timeout.
        exec.set_fake_time((SETVOLUME_TIMEOUT + zx::MonotonicDuration::from_millis(5)).after_now());
        let _ = exec.wake_expired_timers();

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        // Initial volume is returned on timeout.
        assert_matches!(
            exec.run_until_stalled(&mut volume_set_fut).expect("should be ready"),
            Ok(INITIAL_AVRCP_VOLUME)
        );

        // 2. Send a new request to slightly increase volume (e.g., INITIAL_AVRCP_VOLUME + 1).
        let requested_new = INITIAL_AVRCP_VOLUME + 1;
        let volume_set_fut = volume_client.set_volume(requested_new);
        let mut volume_set_fut = pin!(volume_set_fut);

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");
        exec.run_until_stalled(&mut volume_set_fut).expect_pending("should be pending");

        let request_fut = settings_requests.select_next_some();
        let mut request_fut = pin!(request_fut);

        // Requested volume should match the new request not the old failed request.
        match exec.run_until_stalled(&mut request_fut).expect("should be ready") {
            Ok(settings::AudioRequest::Set2 { settings, .. }) => {
                let requested_level = settings.streams.as_ref().unwrap()[0]
                    .user_volume
                    .as_ref()
                    .unwrap()
                    .level
                    .unwrap();

                // Convert back to AVRCP volume for easier assertion.
                let requested_avrcp = (requested_level * 127.0) as u8;

                // We assert that the requested volume matches the new request volume.
                assert_eq!(requested_avrcp, requested_new);
            }
            x => panic!("Expected Ready audio set request and got: {:?}", x),
        };
    }

    #[test]
    fn test_calculate_adjusted_volume_limits() {
        // 1. Underflow: A failed request that asked for 0 and was set to 0.
        let failed_req = SetVolumeRequestData::new(50, 0, 0);
        // Peer tries again with 0 (decrement direction). Should NOT underflow, and stay at 0.
        assert_eq!(failed_req.calculate_adjusted_volume(50, 0), 0);

        // 2. Hijacking: A failed request for 60.
        let failed_req = SetVolumeRequestData::new(50, 60, 61);
        // Peer tries again but asks for 70 (different from failed 60). Should NOT be hijacked.
        assert_eq!(failed_req.calculate_adjusted_volume(50, 70), 70);

        // 3. Nudge Up: A failed request for 60.
        let failed_req = SetVolumeRequestData::new(50, 60, 61);
        // Peer tries again with 60 (same as failed). Current is 50, so it's an increment.
        // Should be nudged up from 61 to 62.
        assert_eq!(failed_req.calculate_adjusted_volume(50, 60), 62);

        // 4. Nudge Down: A failed request for 60.
        let failed_req = SetVolumeRequestData::new(70, 60, 59);
        // Peer tries again with 60 (same as failed). Current is 70, so it's a decrement.
        // Should be nudged down from 59 to 58.
        assert_eq!(failed_req.calculate_adjusted_volume(70, 60), 58);

        // 5. Max Limit: A failed request for 127.
        let failed_req = SetVolumeRequestData::new(50, 127, 127);
        // Peer tries again with 127 (increment direction). Should NOT exceed 127.
        assert_eq!(failed_req.calculate_adjusted_volume(50, 127), 127);
    }

    /// Test that the relay returns the current volume when requested, and completes an
    /// on_volume_changed request when the volume changes locally.
    #[fuchsia::test]
    fn on_volume_change_notification() {
        let mut exec = fasync::TestExecutor::new();
        let (mut settings_requests, avrcp_requests, _stop_sender, relay_fut) = setup_volume_relay();

        let mut relay_fut = pin!(relay_fut);

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        let (volume_client, watch_responder) =
            finish_relay_setup(&mut relay_fut, &mut exec, avrcp_requests, &mut settings_requests);

        let volume_get_fut = volume_client.get_current_volume();
        let mut volume_get_fut = pin!(volume_get_fut);

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        // Volume get should return immediately with the initial volume (0.8 -> 100)
        assert_matches!(
            exec.run_until_stalled(&mut volume_get_fut).expect("should be ready"),
            Ok(INITIAL_AVRCP_VOLUME)
        );

        let volume_hanging_fut = volume_client.on_volume_changed();
        let mut volume_hanging_fut = pin!(volume_hanging_fut);

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        // The OnVolumeChanged request should return immediately the first time.
        assert_matches!(
            exec.run_until_stalled(&mut volume_hanging_fut).expect("should be ready"),
            Ok(INITIAL_AVRCP_VOLUME)
        );

        let volume_hanging_fut = volume_client.on_volume_changed();
        let mut volume_hanging_fut = pin!(volume_hanging_fut);

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        // The next OnVolumeChanged request shouldn't resolve because the volume hasn't changed.
        exec.run_until_stalled(&mut volume_hanging_fut).expect_pending("should be pending");

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        // When a new volume happens as a result, it's returned.
        const CHANGED_MEDIA_VOLUME: f32 = 0.9;
        const CHANGED_AVRCP_VOLUME: u8 = 114; // 0.9 audio settings volume as AVRCP volume.
        respond_to_audio_watch2(watch_responder, CHANGED_MEDIA_VOLUME);

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        assert_matches!(
            exec.run_until_stalled(&mut volume_hanging_fut).expect("should be ready"),
            Ok(CHANGED_AVRCP_VOLUME)
        );

        let _watch_responder = expect_audio_watch2(&mut exec, &mut settings_requests);
    }

    /// Tests the behavior of the VolumeRelay when multiple requests for OnVolumeChanged
    /// updates are requested.
    // TODO(https://fxbug.dev/42131477): This test should be updated to reflect the fact that the channel gets closed
    // when OnVolumeChanged is called twice without a response.
    #[fuchsia::test]
    fn multiple_on_volume_change_notifications() {
        let mut exec = fasync::TestExecutor::new();
        let (mut settings_requests, avrcp_requests, _stop_sender, relay_fut) = setup_volume_relay();

        let mut relay_fut = pin!(relay_fut);

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        // Setup the relay and make two copies of the `volume_client`.
        let (volume_client, watch_responder) =
            finish_relay_setup(&mut relay_fut, &mut exec, avrcp_requests, &mut settings_requests);

        let volume_hanging_fut1 = volume_client.on_volume_changed();
        let mut volume_hanging_fut1 = pin!(volume_hanging_fut1);

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        // The OnVolumeChanged request should return immediately the first time.
        assert_matches!(
            exec.run_until_stalled(&mut volume_hanging_fut1).expect("should be ready"),
            Ok(INITIAL_AVRCP_VOLUME)
        );

        // Make another OnVolumeChanged request.
        let volume_hanging_fut2 = volume_client.on_volume_changed();
        let mut volume_hanging_fut2 = pin!(volume_hanging_fut2);

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        // The next OnVolumeChanged request shouldn't resolve because the volume hasn't changed.
        exec.run_until_stalled(&mut volume_hanging_fut2).expect_pending("should be pending");
        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        // Another request for volume updates.
        let volume_hanging_fut3 = volume_client.on_volume_changed();
        let mut volume_hanging_fut3 = pin!(volume_hanging_fut3);

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        // Respond with a new volume.
        const CHANGED_MEDIA_VOLUME: f32 = 0.9;
        const CHANGED_AVRCP_VOLUME: u8 = 114; // 0.9 audio settings volume as AVRCP volume.
        respond_to_audio_watch2(watch_responder, CHANGED_MEDIA_VOLUME);

        exec.run_until_stalled(&mut relay_fut).expect_pending("should be pending");

        // Both volume update futures should receive the updated avrcp volume.
        assert_matches!(
            exec.run_until_stalled(&mut volume_hanging_fut2).expect("should be ready"),
            Ok(CHANGED_AVRCP_VOLUME)
        );
        assert_matches!(
            exec.run_until_stalled(&mut volume_hanging_fut3).expect("should be ready"),
            Ok(CHANGED_AVRCP_VOLUME)
        );

        let _watch_responder = expect_audio_watch2(&mut exec, &mut settings_requests);
    }
}
