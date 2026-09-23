// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use core::fmt;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use fidl::endpoints::Proxy;
use fidl_fuchsia_bluetooth_avrcp::{
    ControllerEvent, ControllerEventStream, ControllerProxy, Notifications, PeerManagerProxy,
};
use fuchsia_bluetooth::types::PeerId;
use futures::stream::{FusedStream, Stream, StreamExt};
use std::collections::VecDeque;

/// Manages absolute volume control for an active AVRCP connection to a remote peer.
pub struct AbsoluteVolumeControl {
    id: PeerId,
    peer_manager: PeerManagerProxy,
    proxy: ControllerProxy,
    stream: ControllerEventStream,
    set_volume_results: VecDeque<Result<u8, Error>>,
    waker: Option<Waker>,
    is_terminated: bool,
}

impl fmt::Debug for AbsoluteVolumeControl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AbsoluteVolumeControl")
            .field("id", &self.id)
            .field("peer_manager", &self.peer_manager)
            .field("proxy", &self.proxy)
            .field("is_terminated", &self.is_terminated)
            .finish_non_exhaustive()
    }
}

impl AbsoluteVolumeControl {
    /// Creates a new `AbsoluteVolumeControl` for the given peer.
    ///
    /// This will fail if the controller can't be obtained or the notification filter can't be set.
    pub async fn connect(id: PeerId, peer_manager: PeerManagerProxy) -> Result<Self, Error> {
        let (proxy, server_end) =
            fidl::endpoints::create_proxy::<fidl_fuchsia_bluetooth_avrcp::ControllerMarker>();
        peer_manager
            .get_controller_for_target(&id.into(), server_end)
            .await?
            .map_err(|e| format_err!("failed to get controller: {:?}", e))?;
        proxy.set_notification_filter(Notifications::VOLUME, 0)?;
        let stream = proxy.take_event_stream();
        Ok(Self {
            id,
            peer_manager,
            proxy,
            stream,
            set_volume_results: VecDeque::new(),
            waker: None,
            is_terminated: false,
        })
    }

    pub fn peer_id(&self) -> PeerId {
        self.id
    }

    /// Sets the absolute volume on the remote peer.
    ///
    /// `volume_percentage` is a value between 0 and 100.
    ///
    /// This method returns the result of the operation, and also queues the result to be
    /// delivered as an item from the stream.
    pub async fn set_absolute_volume(&mut self, volume_percentage: u8) -> Result<u8, Error> {
        if volume_percentage > 100 {
            return Err(format_err!(
                "volume_percentage ({volume_percentage:}) must be between 0 and 100",
            ));
        }

        let avrcp_volume = (volume_percentage as u16 * 127 / 100) as u8;
        let result = self.proxy.set_absolute_volume(avrcp_volume).await?;

        // Push the result to volume results so clients can get volume update from volume change notification stream.
        self.set_volume_results.push_back(result.clone().map_err(|e| format_err!("{e:?}")));

        if let Some(waker) = self.waker.take() {
            waker.wake();
        }

        result.map_err(|e| format_err!("{e:?}"))
    }
}

impl Stream for AbsoluteVolumeControl {
    type Item = Result<u8, Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if self.is_terminated {
                return Poll::Ready(None);
            }

            if self.proxy.is_closed() {
                self.is_terminated = true;
                return Poll::Ready(None);
            }

            // First, check for results from any completed `set_absolute_volume` commands.
            if let Some(result) = self.set_volume_results.pop_front() {
                return Poll::Ready(Some(result));
            }

            // Next, poll for incoming notifications from the peer.
            match self.stream.poll_next_unpin(cx) {
                Poll::Ready(Some(Ok(ControllerEvent::OnNotification { notification, .. }))) => {
                    // Fire and forget. If this fails, `is_closed()` will be true on the next poll.
                    let _ = self.proxy.notify_notification_handled();

                    if let Some(volume) = notification.volume {
                        return Poll::Ready(Some(Ok(volume)));
                    }
                    // Not a volume notification, so we continue the loop to poll again.
                }
                Poll::Ready(Some(Err(e))) => {
                    self.is_terminated = true;
                    return Poll::Ready(Some(Err(e.into())));
                }
                Poll::Ready(None) => {
                    self.is_terminated = true;
                    return Poll::Ready(None);
                }
                Poll::Pending => {
                    // The underlying stream is pending. Store the waker so that `set_absolute_volume`
                    // can wake us up if a result becomes available.
                    self.waker = Some(cx.waker().clone());
                    return Poll::Pending;
                }
            }
        }
    }
}

impl FusedStream for AbsoluteVolumeControl {
    fn is_terminated(&self) -> bool {
        self.is_terminated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use async_utils::PollExt;
    use fidl::endpoints::{RequestStream, create_proxy_and_stream};
    use fidl_fuchsia_bluetooth_avrcp::{
        ControllerRequest, ControllerRequestStream, PeerManagerMarker, PeerManagerRequest,
        PeerManagerRequestStream,
    };
    use fuchsia_async as fasync;
    use futures::{StreamExt, pin_mut};

    const PEER_ID: PeerId = PeerId(1);

    #[track_caller]
    fn set_up(
        exec: &mut fasync::TestExecutor,
    ) -> (AbsoluteVolumeControl, PeerManagerRequestStream, ControllerRequestStream) {
        let (peer_manager_proxy, mut peer_manager_stream) =
            create_proxy_and_stream::<PeerManagerMarker>();

        let connect_fut = AbsoluteVolumeControl::connect(PEER_ID, peer_manager_proxy);
        pin_mut!(connect_fut);

        exec.run_until_stalled(&mut connect_fut).expect_pending("should be pending");

        // The server should receive a `GetControllerForTarget` request.
        let mut request_fut = peer_manager_stream.next();
        let request = exec.run_until_stalled(&mut request_fut).expect("stream item");
        let mut controller_stream = match request {
            Some(Ok(PeerManagerRequest::GetControllerForTarget { peer_id, client, responder })) => {
                assert_eq!(peer_id, PEER_ID.into());
                responder.send(Ok(())).expect("should succeed");
                client.into_stream()
            }
            r => panic!("Expected GetControllerForTarget request, got {:?}", r),
        };

        let avc =
            exec.run_until_stalled(&mut connect_fut).expect("should have succeeded").expect("ok");

        let mut request_fut = controller_stream.next();
        let request = exec.run_until_stalled(&mut request_fut).expect("stream item");
        match request {
            Some(Ok(ControllerRequest::SetNotificationFilter { notifications, .. })) => {
                assert_eq!(notifications, Notifications::VOLUME);
            }
            r => panic!("Expected SetNotificationFilter request, got {:?}", r),
        }
        assert_eq!(avc.peer_id(), PEER_ID);

        (avc, peer_manager_stream, controller_stream)
    }

    #[fuchsia::test]
    fn test_set_absolute_volume() {
        let mut exec = fasync::TestExecutor::new();
        let (mut avc, _peer_manager_stream, mut controller_stream) = set_up(&mut exec);

        let requested_vol_percent = 50;
        let expected_avrcp_volume = 63; // 50 * 127 / 100
        let expected_response_volume = 64;

        // No volume update yet.
        exec.run_until_stalled(&mut avc.next()).expect_pending("pending");

        {
            let set_vol_fut = avc.set_absolute_volume(requested_vol_percent);
            pin_mut!(set_vol_fut);
            exec.run_until_stalled(&mut set_vol_fut).expect_pending("pending");

            // The Controller server should have received a `SetAbsoluteVolume` request.
            let request_fut = controller_stream.next();
            pin_mut!(request_fut);
            let request = exec.run_until_stalled(&mut request_fut).expect("stream item");
            match request {
                Some(Ok(ControllerRequest::SetAbsoluteVolume { requested_volume, responder })) => {
                    assert_eq!(requested_volume, expected_avrcp_volume);
                    responder.send(Ok(expected_response_volume)).unwrap();
                }
                r => panic!("Expected SetAbsoluteVolume request, got {:?}", r),
            }

            // The future should complete with the result.
            let result = exec.run_until_stalled(&mut set_vol_fut).expect("future should complete");
            assert_eq!(result.unwrap(), expected_response_volume);
        }

        let stream_result =
            exec.run_until_stalled(&mut avc.next()).expect("stream should have item");
        assert_eq!(stream_result.unwrap().unwrap(), expected_response_volume);
    }

    #[fuchsia::test]
    fn test_set_absolute_volume_out_of_range() {
        let mut exec = fasync::TestExecutor::new();
        let (mut avc, _peer_manager_stream, _controller_stream) = set_up(&mut exec);

        let requested_vol_percent = 101;

        let set_vol_fut = avc.set_absolute_volume(requested_vol_percent);
        pin_mut!(set_vol_fut);
        let result = exec.run_until_stalled(&mut set_vol_fut).expect("future should complete");
        assert!(result.is_err());
    }

    #[fuchsia::test]
    fn test_volume_notification_stream() {
        let mut exec = fasync::TestExecutor::new();
        let (mut avc, _peer_manager_stream, mut controller_stream) = set_up(&mut exec);

        // No volume change yet.
        let vol_change_fut = avc.next();
        pin_mut!(vol_change_fut);
        exec.run_until_stalled(&mut vol_change_fut).expect_pending("pending");

        // Mock the server sending a volume notification.
        let event =
            fidl_fuchsia_bluetooth_avrcp::Notification { volume: Some(100), ..Default::default() };
        controller_stream.control_handle().send_on_notification(1000, &event).unwrap();

        let new_vol = exec
            .run_until_stalled(&mut vol_change_fut)
            .expect("stream item")
            .expect("some")
            .expect("ok");
        assert_eq!(new_vol, 100);

        // Should have acknowledged the notification.
        let mut request_fut = controller_stream.next();
        let request = exec.run_until_stalled(&mut request_fut).expect("stream item").expect("some");
        assert_matches!(request, Ok(ControllerRequest::NotifyNotificationHandled { .. }));
    }

    #[fuchsia::test]
    fn test_stream_terminates() {
        let mut exec = fasync::TestExecutor::new();
        let (avc, _peer_manager_stream, controller_stream) = set_up(&mut exec);

        pin_mut!(avc);
        let mut vol_change_fut = avc.next();

        // The future should be pending before any events.
        assert!(exec.run_until_stalled(&mut vol_change_fut).is_pending());

        // Drop the server-side stream. This will cause the client stream to terminate
        // gracefully on its next poll.
        drop(controller_stream);

        // Poll will now see the closed channel and return Ready with None.
        assert!(exec.run_until_stalled(&mut vol_change_fut).expect("ready").is_none());
        assert!(avc.is_terminated());
    }
}
