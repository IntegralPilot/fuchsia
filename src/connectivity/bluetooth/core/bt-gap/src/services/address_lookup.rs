// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{bail, Error};
use fidl_fuchsia_bluetooth::Address;
use futures::stream::TryStreamExt;

use fidl_fuchsia_bluetooth_sys::{
    AddressLookupLookupRequest, AddressLookupRequest, AddressLookupRequestStream, LookupError,
};

use crate::host_dispatcher::HostDispatcher;

pub async fn run(hd: HostDispatcher, mut stream: AddressLookupRequestStream) -> Result<(), Error> {
    while let Some(request) = stream.try_next().await? {
        handler(hd.clone(), request).await?;
    }
    Ok(())
}

async fn handler(hd: HostDispatcher, request: AddressLookupRequest) -> Result<(), Error> {
    match request {
        AddressLookupRequest::Lookup {
            payload: AddressLookupLookupRequest { peer_id: Some(peer_id), .. },
            responder,
        } => {
            let addr: Option<Address> = hd.get_peer_address(peer_id.into()).map(Into::into);
            let resp = addr.as_ref().ok_or(LookupError::NotFound);
            responder.send(resp)?;
            Ok(())
        }
        AddressLookupRequest::Lookup { responder, .. } => {
            responder.send(Err(LookupError::MissingArgument))?;
            Ok(())
        }
        AddressLookupRequest::_UnknownMethod { ordinal, method_type, .. } => {
            bail!("Unknown address lookup method ({ordinal} - {method_type:?})")
        }
    }
}
