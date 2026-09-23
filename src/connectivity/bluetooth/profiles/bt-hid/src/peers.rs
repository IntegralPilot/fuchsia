// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_utils::stream::FutureMap;
use fidl_fuchsia_bluetooth_bredr as bredr;
use fuchsia_bluetooth::types::PeerId;
use futures::StreamExt;
use log::{debug, error, info};
use profile_client::{ProfileClient, ProfileEvent};
use std::collections::hash_map::Entry;

use crate::peer_task::PeerTask;

/// Struct containing all known peers and the profile client.
pub struct Peers {
    profile_client: ProfileClient,
    profile_proxy: bredr::ProfileProxy,
    peers: FutureMap<PeerId, PeerTask>,
}

// TODO(https://fxbug.dev/42163927) Clean up when a peer task finishes.
impl Peers {
    pub fn new(profile_client: ProfileClient, profile_proxy: bredr::ProfileProxy) -> Self {
        Self { profile_client, profile_proxy, peers: FutureMap::new() }
    }

    pub async fn run(&mut self) {
        loop {
            info!("In main loop awaiting profile events.");
            match self.profile_client.next().await {
                Some(Ok(event)) => {
                    info!("Received profile event: {:?}", event);
                    self.handle_profile_event(event).await;
                }
                Some(Err(e)) => {
                    error!("Error encountered wating for profile events: {}.", e);
                    break;
                }
                None => {
                    debug!("Profile event stream terminated.");
                    break;
                }
            }
        }
    }

    async fn handle_profile_event(&mut self, event: ProfileEvent) {
        let peer_id = event.peer_id();
        debug!("Handling profile event: {:?}", event);

        let task = self.make_task_if_nonexistent(peer_id);

        let send_result = task.handle_profile_event(event).await;
        if let Err(err) = send_result {
            // At this point we've just created a task but can't communicate with it.  This is an unrecoverable error.
            error!("Unable to send profile event to peer {:} with error {:?}", peer_id, err);
            let _ = self.peers.remove(&peer_id);
        }
    }

    fn make_task_if_nonexistent(&mut self, peer_id: PeerId) -> &mut PeerTask {
        match self.peers.inner().entry(peer_id) {
            Entry::Occupied(task_entry) => Ref::into_mut(task_entry),
            Entry::Vacant(task_entry) => {
                let task = PeerTask::spawn_new(peer_id, self.profile_proxy.clone());
                task_entry.insert(Box::pin(task))
            }
        }
    }

    #[cfg(test)]
    fn get_peer_map(&mut self) -> &mut FutureMap<PeerId, PeerTask> {
        &mut self.peers
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_test_helpers::run_while;
    use async_utils::PollExt;
    use {fuchsia_async as fasync, test_profile_server};

    #[fuchsia::test]
    fn search_result_creates_peer_task() {
        // Set up peers.
        let mut exec = fasync::TestExecutor::new();
        let test_profile_server::TestProfileServerEndpoints { proxy, client, mut test_server } =
            test_profile_server::TestProfileServer::new(
                None,                                                             // Advertisement
                Some(bredr::ServiceClassProfileIdentifier::HumanInterfaceDevice), // Search
            );
        let mut peers = Peers::new(client, proxy);

        assert_eq!(peers.get_peer_map().inner().len(), 0);

        // A service found event causes a task to be added to the peers map.
        {
            let peers_fut = Box::pin(peers.run());

            let (_, peers_fut) = run_while(&mut exec, peers_fut, test_server.expect_search());
            let (service_found_result, mut peers_fut) = run_while(
                &mut exec,
                peers_fut,
                test_server.send_service_found(PeerId(1), None, Vec::new()),
            );
            service_found_result.expect("Service found result");

            exec.run_until_stalled(&mut peers_fut).expect_pending("Handling connected first 1");
        }
        assert_eq!(peers.get_peer_map().inner().len(), 1);

        // A second service found event for the same peer doesn't add a new task to the peers map.
        {
            let peers_fut = Box::pin(peers.run());

            let (service_found_result, mut peers_fut) = run_while(
                &mut exec,
                peers_fut,
                test_server.send_service_found(PeerId(1), None, Vec::new()),
            );
            service_found_result.expect("Service found result");

            exec.run_until_stalled(&mut peers_fut).expect_pending("Handling connected second 1");
        }
        assert_eq!(peers.get_peer_map().inner().len(), 1);

        // A service found event for a second peer causes a task to be added to the peers map.
        {
            let peers_fut = Box::pin(peers.run());

            let (service_found_result, mut peers_fut) = run_while(
                &mut exec,
                peers_fut,
                test_server.send_service_found(PeerId(2), None, Vec::new()),
            );
            service_found_result.expect("Service found result");

            exec.run_until_stalled(&mut peers_fut).expect_pending("Handling connected 2");
        }
        assert_eq!(peers.get_peer_map().inner().len(), 2);
    }
}
