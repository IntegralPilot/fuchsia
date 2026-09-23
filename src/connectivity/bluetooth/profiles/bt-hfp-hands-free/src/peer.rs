// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, format_err};
use bt_hfp::{audio, sco};
use bt_rfcomm::profile as rfcomm;
use fidl::endpoints::{ClientEnd, create_request_stream};
use fuchsia_bluetooth::profile::ProtocolDescriptor;
use fuchsia_bluetooth::types::{Channel, PeerId};
use fuchsia_sync::Mutex;
use futures::FutureExt;
use log::{debug, info, warn};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use {
    fidl_fuchsia_bluetooth as fidl_bt, fidl_fuchsia_bluetooth_bredr as bredr,
    fidl_fuchsia_bluetooth_hfp as fidl_hfp, fuchsia_async as fasync,
};

use peer_task::PeerTask;

use crate::config::HandsFreeFeatureSupport;

mod ag_indicators;
mod at_connection;
mod calls;
mod hf_indicators;
mod indicated_state;
mod parse_cind_test;
mod peer_task;
mod procedure;
mod procedure_manager;
mod procedure_manipulated_state;

/// Represents a Bluetooth peer that supports the AG role. Manages the Service
/// Level Connection, Audio Connection, and FIDL APIs.
pub struct Peer {
    peer_id: PeerId,
    hf_features: HandsFreeFeatureSupport,
    profile_proxy: bredr::ProfileProxy,
    sco_connector: sco::Connector,
    audio_control: Arc<Mutex<Box<dyn audio::Control>>>,
    /// The processing task for data received from the remote peer over RFCOMM
    /// or FIDL APIs.
    /// This value is None if there is no RFCOMM channel present.
    /// If set, there is no guarantee that the RFCOMM channel is open.
    task: Option<fasync::Task<PeerId>>,
    waker: Option<Waker>,
}

impl Future for Peer {
    type Output = PeerId;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        match self.task.as_mut() {
            None => {
                debug!("Task for peer {} polled without async task set.", self.peer_id);
                self.waker = Some(context.waker().clone());
                Poll::Pending
            }
            Some(task) => task.poll_unpin(context),
        }
    }
}

impl Peer {
    pub fn new(
        peer_id: PeerId,
        hf_features: HandsFreeFeatureSupport,
        profile_proxy: bredr::ProfileProxy,
        sco_connector: sco::Connector,
        audio_control: Arc<Mutex<Box<dyn audio::Control>>>,
    ) -> Self {
        Self {
            peer_id,
            hf_features,
            profile_proxy,
            sco_connector,
            audio_control,
            task: None,
            waker: None,
        }
    }

    /// Handle a PeerConnected ProfileEvent.  This creates a new peer task, so return the
    /// PeerHandlerProxy appropriate to it.
    pub fn handle_peer_connected(
        &mut self,
        rfcomm: Channel,
    ) -> ClientEnd<fidl_hfp::PeerHandlerMarker> {
        if self.task.take().is_some() {
            info!(peer:% = self.peer_id; "Shutting down existing task on incoming RFCOMM channel");
        }

        let (peer_handler_client_end, peer_handler_request_stream) =
            create_request_stream::<fidl_hfp::PeerHandlerMarker>();

        let task = PeerTask::spawn(
            self.peer_id,
            self.hf_features,
            peer_handler_request_stream,
            rfcomm,
            self.sco_connector.clone(),
            self.audio_control.clone(),
        );
        self.task = Some(task);
        self.awaken();

        peer_handler_client_end
    }

    fn awaken(&mut self) {
        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
    }

    /// Handle a SearchResult ProfileEvent.  If a new peer task is created,
    /// return the PeerHandlerProxy appropriate to it.  Returns Err(_) in the
    /// case of an error, Ok(None) in the case a task is already running for
    /// this peeer, or  Ok(Some(client_end)) if a new task was created.
    pub async fn handle_search_result(
        &mut self,
        protocol: Option<Vec<ProtocolDescriptor>>,
    ) -> Result<Option<ClientEnd<fidl_hfp::PeerHandlerMarker>>> {
        if self.task.is_some() {
            info!(peer:% = self.peer_id; "Already connected, ignoring search result");
            return Ok(None);
        }
        // If we haven't started the task, connect to the peer and do so.
        info!(peer:% = self.peer_id; "Connecting RFCOMM.");

        let rfcomm_result = self.connect_from_protocol(protocol).await;
        let rfcomm = match rfcomm_result {
            Ok(rfcomm) => rfcomm,
            Err(err) => {
                warn!(peer:% = self.peer_id, err:?; "Unable to connect RFCOMM to peer.");
                return Err(err);
            }
        };

        let (peer_handler_client_end, peer_handler_request_stream) =
            create_request_stream::<fidl_hfp::PeerHandlerMarker>();

        let task = PeerTask::spawn(
            self.peer_id,
            self.hf_features,
            peer_handler_request_stream,
            rfcomm,
            self.sco_connector.clone(),
            self.audio_control.clone(),
        );
        self.task = Some(task);
        self.awaken();

        Ok(Some(peer_handler_client_end))
    }

    async fn connect_from_protocol(
        &self,
        protocol_option: Option<Vec<ProtocolDescriptor>>,
    ) -> Result<Channel> {
        let protocol = protocol_option.ok_or_else(|| {
            format_err!("Got no protocols for peer {:} in search result.", self.peer_id)
        })?;

        let server_channel_option = rfcomm::server_channel_from_protocol(&protocol);
        let server_channel = server_channel_option.ok_or_else(|| {
            format_err!(
                "Search result received for non-RFCOMM protocol {:?} from peer {:}.",
                protocol,
                self.peer_id
            )
        })?;

        let params = bredr::ConnectParameters::Rfcomm(bredr::RfcommParameters {
            channel: Some(server_channel.into()),
            ..bredr::RfcommParameters::default()
        });
        let peer_id: fidl_bt::PeerId = self.peer_id.into();

        let fidl_channel_result_result = self.profile_proxy.connect(&peer_id, &params).await;
        let fidl_channel = fidl_channel_result_result
            .map_err(|e| {
                format_err!(
                    "Unable to connect RFCOMM to peer {:} with FIDL error {:?}",
                    self.peer_id,
                    e
                )
            })?
            .map_err(|e| {
                format_err!(
                    "Unable to connect RFCOMM to peer {:} with Bluetooth error {:?}",
                    self.peer_id,
                    e
                )
            })?;

        // Convert a bredr::Channel into a fuchsia_bluetooth::types::Channel
        let channel = fidl_channel.try_into()?;

        Ok(channel)
    }

    pub fn task_exists(&self) -> bool {
        self.task.is_some()
    }
}
