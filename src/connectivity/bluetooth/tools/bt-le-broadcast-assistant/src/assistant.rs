// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bt_bass::client::error::Error as BassClientError;
use bt_bass::client::event::Event as BassEvent;
use bt_broadcast_assistant::debug::*;
use bt_common::debug_command::CommandRunner;
use bt_gatt::pii::GetPeerAddr;
use fuchsia_async as fasync;
use futures::channel::oneshot;
use futures::stream::FusedStream;
use futures::{Stream, StreamExt};

use crate::address_lookup::LocalPeerAddrCache;

enum AssistantStatus {
    // Broadcast Assistant has started scanning for broadcast sources.
    Started {
        event_processing_task: fasync::Task<()>,
        stop_event_processing_tx: oneshot::Sender<()>,
    },
    NotStarted,
}

pub(crate) struct AssistantState<T: bt_gatt::GattTypes, R: GetPeerAddr> {
    // Broadcast assistant we use to discover and connect to peers.
    pub(crate) debug: AssistantDebug<T, R>,

    // Cache for peer addresses, only used when `--use-static-address` is enabled.
    pub(crate) local_cache: Option<LocalPeerAddrCache>,

    // Task to print scan delegator peer events.
    peer_event_task: Option<fasync::Task<()>>,

    // The status of the broadcast assistant.
    status: AssistantStatus,
}

async fn printout_bass_events(
    bass_stream: impl Stream<Item = Result<BassEvent, BassClientError>> + FusedStream,
) {
    futures::pin_mut!(bass_stream);
    while let Some(res) = bass_stream.next().await {
        match res {
            Ok(event) => println!("\n\t[BASS Event]\n\t{:#?}\n", event),
            Err(e) => {
                eprintln!("\n\t[BASS Error]\n\t{:#?}\n", e);
                break;
            }
        }
    }
}

impl<T: bt_gatt::GattTypes + 'static, R: GetPeerAddr> AssistantState<T, R>
where
    T::ScanResultStream: Send + Unpin,
    T::NotificationStream: Send,
{
    pub fn new(debug: AssistantDebug<T, R>, local_cache: Option<LocalPeerAddrCache>) -> Self {
        AssistantState {
            debug,
            local_cache,
            peer_event_task: None,
            status: AssistantStatus::NotStarted,
        }
    }

    pub fn start_broadcast_assistant(&mut self) {
        if let AssistantStatus::Started { .. } = self.status {
            return;
        }
        println!("\nStarting Broadcast Assistant to scan for Broadcast Sources...\n");
        let (tx, rx) = oneshot::channel();
        let event_stream = self.debug.start().unwrap();
        let task = fasync::Task::local(async move {
            let mut stream = event_stream.take_until(rx);
            while let Some(res) = stream.next().await {
                match res {
                    Ok(event) => println!("\n\t[Broadcast Assistant Event]\n\t{:#?}\n", event),
                    Err(e) => {
                        println!("\n\t[Broadcast Assistant Error]\n\t{:#?}\n", e);
                        break;
                    }
                }
            }
            println!("\nBroadcast Assistant stopped scanning for Broadcast Sources.\n");
        });
        self.status =
            AssistantStatus::Started { event_processing_task: task, stop_event_processing_tx: tx };
    }

    pub async fn stop_broadcast_assistant(&mut self) {
        if let AssistantStatus::Started { event_processing_task, stop_event_processing_tx } =
            std::mem::replace(&mut self.status, AssistantStatus::NotStarted)
        {
            // Signal the event processing task to stop.
            let _ = stop_event_processing_tx.send(());
            // Wait for the task to complete.
            event_processing_task.await;
        }
    }
}

pub async fn assistant_cmd<T: bt_gatt::GattTypes + 'static, R: GetPeerAddr + 'static>(
    state: &mut AssistantState<T, R>,
    cmd: AssistantCmd,
    args: Vec<String>,
) where
    <T as bt_gatt::GattTypes>::ScanResultStream: std::marker::Send,
    <T as bt_gatt::GattTypes>::NotificationStream: std::marker::Send,
{
    if let Err(e) = state.debug.run(cmd, args).await {
        eprintln!("\n[Error] Error running assistant: {e:#?}\n");
        return;
    }

    if let Ok(peer_event_stream) = state.debug.take_connected_peer_event_stream() {
        let event_task = fasync::Task::local(async move {
            let _ = printout_bass_events(peer_event_stream).await;
            eprintln!("\n\t[Notice] Connected scan delegator peer event stream terminated!\n");
        });
        state.peer_event_task = Some(event_task);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bt_common::core::AddressType;
    use bt_gatt::pii::StaticPeerAddr;
    use bt_gatt_fuchsia::{Central, FuchsiaTypes};

    #[fuchsia::test]
    async fn test_assistant_state_new() {
        let (central_proxy, _central_mock) =
            fidl::endpoints::create_proxy_and_stream::<fidl_fuchsia_bluetooth_le::CentralMarker>();
        let central = Central::new(central_proxy);
        let peer_addr_getter = StaticPeerAddr::new([0; 6], AddressType::Public);
        let debug = AssistantDebug::<FuchsiaTypes, _>::new(central, peer_addr_getter);
        let _state = AssistantState::new(debug, None);
    }
}
