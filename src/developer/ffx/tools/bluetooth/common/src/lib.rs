// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ::fho::Result;
use fdomain_fuchsia_bluetooth_sys::{PairingDelegateRequest, PairingDelegateRequestStream};
use ffx_writer::{SimpleWriter, ToolIO as _};
use fuchsia_bluetooth::types::PeerId;
use futures::StreamExt;
use regex::Regex;
use std::fmt;
use std::str::FromStr;

/// A Bluetooth MAC address: 6 bytes written in hexadecimal and separated by colons.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct BdAddr(pub String);

impl FromStr for BdAddr {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let address_pattern = Regex::new(r"^([0-9A-Fa-f]{2}[:-]){5}([0-9A-Fa-f]{2})$")
            .expect("Could not compile MAC address regex pattern.");
        if address_pattern.is_match(s) {
            Ok(Self(s.to_string()))
        } else {
            Err("Not a valid MAC address".to_string())
        }
    }
}

impl fmt::Display for BdAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerIdOrAddr {
    PeerId(PeerId),
    BdAddr(BdAddr),
}

impl FromStr for PeerIdOrAddr {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Ok(addr) = BdAddr::from_str(s) {
            return Ok(PeerIdOrAddr::BdAddr(addr));
        }
        if let Ok(id) = PeerId::from_str(s) {
            return Ok(PeerIdOrAddr::PeerId(id));
        }
        Err("Not a valid Peer ID or MAC address".to_string())
    }
}
impl fmt::Display for PeerIdOrAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PeerIdOrAddr::PeerId(id) => write!(f, "{}", id),
            PeerIdOrAddr::BdAddr(addr) => write!(f, "{}", addr),
        }
    }
}

pub async fn handle_pairing_delegate_requests(
    mut stream: PairingDelegateRequestStream,
    peer_id: Option<PeerId>,
    writer: &mut SimpleWriter,
) -> Result<()> {
    let mut pairing_occurred = false;
    while let Some(req) = stream.next().await {
        match req {
            Ok(event) => match event {
                PairingDelegateRequest::OnPairingComplete { id, success, control_handle: _ } => {
                    writer.line(format!("Pairing complete for peer {id:?}: success={success}"))?;
                    if success && peer_id.map_or(true, |target| target.0 == id.value) {
                        pairing_occurred = true;
                        break;
                    }
                }
                PairingDelegateRequest::OnPairingRequest {
                    peer,
                    method: _,
                    displayed_passkey: _,
                    responder,
                } => {
                    if let Some(target_peer_id) = peer_id {
                        let request_peer_id = peer.id.map(|pid| pid.value);
                        if request_peer_id != Some(target_peer_id.0) {
                            writer.line(format!(
                                "Rejecting pairing request from non-target peer: {peer:?}"
                            ))?;
                            let _ = responder.send(false, 0);
                            continue;
                        }
                    }
                    writer.line(format!("Accepting pairing request from peer {peer:?}"))?;
                    let _ = responder.send(true, 0);
                }
                _ => {}
            },
            Err(err) => return Err(fho::Error::Unexpected(anyhow::anyhow!("FIDL error: {err}"))),
        }
    }

    if pairing_occurred {
        Ok(())
    } else {
        Err(fho::Error::Unexpected(anyhow::anyhow!(
            "Unable to allow pairing: Pairing delegate closed without completing a pairing (another delegate may already be active)"
        )))
    }
}
