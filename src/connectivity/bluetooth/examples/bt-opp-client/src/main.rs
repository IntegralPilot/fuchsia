// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{format_err, Error};
use bt_obex::client::ObexClient;
use bt_obex::header::{Header, HeaderSet};
use bt_obex::profile::{connect_to_obex_service, parse_obex_search_result};
use fidl_fuchsia_bluetooth_bredr as bredr;
use fuchsia_bluetooth::profile::{Attribute, ProtocolDescriptor};
use fuchsia_bluetooth::types::PeerId;
use fuchsia_component::client::connect_to_protocol;
use futures::stream::StreamExt;
use log::{info, warn};
use profile_client::{ProfileClient, ProfileEvent};

/// An example data buffer to be sent to the remote peer.
// TODO(https://fxbug.dev/42083292): Replace this with a well-formatted OPP type.
const EXAMPLE_DATA: [u8; 10] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];

async fn write_example_data(id: PeerId, client: ObexClient) -> Result<(), Error> {
    // An OPP client write requires sending Connect, Put, Disconnect.
    info!(id:%; "Attempting to initiate OBEX session");
    let input = HeaderSet::from_header(Header::Description("example OPP client service".into()));
    let response = client.connect(input).await?;
    info!(id:%; "connected to OBEX server: {response:?}");

    let put_operation = client.put()?;
    let response = put_operation.write_final(&EXAMPLE_DATA, HeaderSet::default()).await?;
    info!(id:%; "wrote data: {response:?}");

    // Either calling `ObexClient::disconnect` or dropping `client` signals disconnection from the
    // OBEX service.
    let response = client.disconnect(HeaderSet::default()).await?;
    info!(id:%; "disconnected session: {response:?}");
    Ok(())
}

async fn handle_profile_events(
    profile: bredr::ProfileProxy,
    mut profile_client: ProfileClient,
) -> Result<(), Error> {
    while let Some(event) = profile_client.next().await {
        match event.map_err(|e| format_err!("{e:?}"))? {
            ProfileEvent::SearchResult { id, protocol, attributes, .. } => {
                let protocol = protocol
                    .unwrap_or(Vec::new())
                    .iter()
                    .map(|p| ProtocolDescriptor::try_from(p))
                    .collect::<Result<Vec<_>, _>>()?;
                let attributes = attributes
                    .iter()
                    .map(|a| Attribute::try_from(a))
                    .collect::<Result<Vec<_>, _>>()?;
                info!(id:%; "Found search result: {protocol:?}");
                let Some(connect_parameters) = parse_obex_search_result(&protocol, &attributes)
                else {
                    warn!(id:%; "No OBEX service in protocol");
                    continue;
                };
                let client = match connect_to_obex_service(id, &profile, connect_parameters).await {
                    Ok(obex_client) => {
                        info!(id:%; "successfully connected to OPP service");
                        obex_client
                    }
                    Err(e) => {
                        warn!(id:%; "couldn't connect to OPP service: {e:?}");
                        continue;
                    }
                };
                // TODO(https://fxbug.dev/42083292): Update this to do whatever OBEX operation is specified
                // in the config.
                match write_example_data(id, client).await {
                    Ok(_) => info!("Finished writing data"),
                    Err(e) => warn!(id:%; "couldn't write data {e:?}"),
                }
            }
            ProfileEvent::PeerConnected { id, protocol, .. } => {
                return Err(format_err!("Unexpected connection from {id} ({protocol:?}"));
            }
        }
    }
    Err(format_err!("ProfileClient terminated"))
}

#[fuchsia::main(logging_tags = ["bt-opp-client"])]
async fn main() -> Result<(), Error> {
    info!("Starting Object Push client");
    let profile = connect_to_protocol::<bredr::ProfileMarker>()?;
    let mut profile_client = ProfileClient::new(profile.clone());
    profile_client.add_search(bredr::ServiceClassProfileIdentifier::ObexObjectPush, None)?;
    let result = handle_profile_events(profile, profile_client).await;
    info!("OPP client finished: {result:?}");
    Ok(())
}
