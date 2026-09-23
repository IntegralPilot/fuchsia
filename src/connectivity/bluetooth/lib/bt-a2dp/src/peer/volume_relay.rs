// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bt_avrcp_vol_control::AbsoluteVolumeControl;
use fidl_fuchsia_bluetooth_avrcp as avrcp;
use futures::StreamExt;
use log::{debug, info, trace, warn};

use fuchsia_bluetooth::types::PeerId;

/// Runs the AVRCP Absolute Volume Relay for the A2DP Source role.
///
/// This is used when the Fuchsia device is the A2DP Source (sending audio) and the peer
/// device is the A2DP Sink (receiving audio). In this configuration, for AVRCP Absolute
/// Volume control, the Fuchsia device acts as the AVRCP Controller, while the peer device
/// acts as the AVRCP Target.
///
/// It will initiate an AVRCP connection to control and monitor the peer's volume.
pub async fn run_avrcp_volume_relay(peer_id: PeerId, avrcp_svc: avrcp::PeerManagerProxy) {
    let Ok(mut volume_control) = AbsoluteVolumeControl::connect(peer_id, avrcp_svc).await else {
        warn!(peer_id:%; "Failed to connect AVRCP volume control for peer");
        return;
    };

    debug!(peer_id:%; "Initialized AbsoluteVolumeControl for peer");
    // TODO(b/524288927): Fully implement AVRCP absolute volume control for the A2DP Source role
    // by synchronizing the peer's volume with Fuchsia's local system volume.
    // For now, we only consume the stream to keep the connection active.
    while let Some(res) = volume_control.next().await {
        trace!(peer_id:%; "AVRCP volume update for peer: {res:?}");
    }
    info!(peer_id:%; "AVRCP volume control disconnected for peer");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connected_peers::ConnectedPeers;
    use async_utils::PollExt;
    use bt_avdtp as avdtp;
    use bt_channel_test_support::{Transport, create_test_channels};
    use fidl::endpoints;
    use fidl_fuchsia_bluetooth_bredr as bredr;
    use fuchsia_async as fasync;
    use fuchsia_bluetooth::types::PeerId;
    use futures::task::Poll;
    use std::sync::Arc;
    use test_case::test_case;

    #[fuchsia::test]
    fn test_run_avrcp_volume_relay_triggers_connect() {
        let mut exec = fasync::TestExecutor::new();
        let (avrcp_proxy, mut avrcp_stream) =
            endpoints::create_proxy_and_stream::<avrcp::PeerManagerMarker>();

        let peer_id = PeerId(1);

        // Start the runner task.
        let mut relay_fut = std::pin::pin!(run_avrcp_volume_relay(peer_id, avrcp_proxy));

        // We expect GetControllerForTarget.
        let mut get_controller_fut = avrcp_stream.select_next_some();

        // Run the relay future until stalled. It should block on connecting.
        assert!(exec.run_until_stalled(&mut relay_fut).is_pending());

        match exec.run_until_stalled(&mut get_controller_fut) {
            Poll::Ready(Ok(avrcp::PeerManagerRequest::GetControllerForTarget {
                peer_id: req_peer_id,
                client: _,
                responder,
            })) => {
                assert_eq!(req_peer_id, peer_id.into());
                responder.send(Ok(())).expect("should send response");
            }
            x => panic!("Expected GetControllerForTarget request, got {x:?}"),
        }
    }

    #[test_case(Transport::Socket ; "socket")]
    #[test_case(Transport::Fidl ; "fidl")]
    #[fuchsia::test]
    fn test_volume_relay_task_cancelled_when_peer_is_dropped(transport: Transport) {
        let mut exec = fasync::TestExecutor::new();
        let (peers, _profile_stream) = setup_connected_peers();
        let (avrcp_proxy, mut avrcp_stream) =
            endpoints::create_proxy_and_stream::<avrcp::PeerManagerMarker>();

        let peer_id = PeerId(1);
        let (remote, signaling) = create_test_channels(transport);
        let remote_peer = avdtp::Peer::new(remote);
        let remote_events = remote_peer.take_request_stream();

        let mut connected_fut = std::pin::pin!(peers.connected(peer_id.clone(), signaling, None));
        let weak_peer = exec.run_until_stalled(&mut connected_fut).expect("ready").expect("okay");
        let peer = weak_peer.upgrade().expect("peer should exist");

        // Start the runner task.
        let task = fasync::Task::spawn(run_avrcp_volume_relay(peer_id, avrcp_proxy));
        peer.set_volume_relay_task(task);

        // We expect GetControllerForTarget.
        let mut get_controller_fut = avrcp_stream.select_next_some();

        // Respond to GetControllerForTarget to establish connection
        let mut controller_request_stream = match exec.run_until_stalled(&mut get_controller_fut) {
            Poll::Ready(Ok(avrcp::PeerManagerRequest::GetControllerForTarget {
                peer_id: req_peer_id,
                client,
                responder,
            })) => {
                assert_eq!(req_peer_id, peer_id.into());
                responder.send(Ok(())).expect("should send response");
                client.into_stream()
            }
            x => panic!("Expected GetControllerForTarget request, got {x:?}"),
        };

        // Verify that the controller channel is not closed.
        assert_matches::assert_matches!(
            exec.run_until_stalled(&mut controller_request_stream.next()),
            Poll::Pending | Poll::Ready(Some(_))
        );

        // Now, we drop the strong references to `peer`.
        drop(peer);
        // And we close the remote end of the signaling channel to trigger peer disconnection and detachment
        drop(remote_events);
        drop(remote_peer);

        // Run the executor to let the closed_fut and detach task run.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

        // Now, because the Peer was dropped, the volume relay task should have been cancelled,
        // which drops the Controller client, so `controller_request_stream` should be closed!
        assert_matches::assert_matches!(
            exec.run_until_stalled(&mut controller_request_stream.next()),
            Poll::Ready(None)
        );
    }

    fn setup_connected_peers() -> (Arc<ConnectedPeers>, bredr::ProfileRequestStream) {
        let (proxy, stream) = endpoints::create_proxy_and_stream::<bredr::ProfileMarker>();
        let peers = Arc::new(ConnectedPeers::new(
            crate::stream::StreamsBuilder::default(),
            crate::permits::Permits::new(1),
            proxy,
            None,
            bt_metrics::MetricsLogger::default(),
        ));
        (peers, stream)
    }
}
