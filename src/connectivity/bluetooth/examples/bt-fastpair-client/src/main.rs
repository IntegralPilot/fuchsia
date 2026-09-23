// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use fidl::endpoints::create_request_stream;
use fidl_fuchsia_bluetooth_fastpair::{
    ProviderMarker, ProviderWatcherMarker, ProviderWatcherRequestStream,
};
use fidl_fuchsia_bluetooth_sys::{
    InputCapability, OutputCapability, PairingDelegateMarker, PairingDelegateRequest,
    PairingDelegateRequestStream, PairingMarker,
};
use fuchsia_bluetooth::types::PeerId;
use fuchsia_component::client::connect_to_protocol;
use futures::stream::TryStreamExt;
use futures::{select, FutureExt};
use log::{info, warn};
use std::pin::pin;

async fn process_provider_events(mut stream: ProviderWatcherRequestStream) -> Result<(), Error> {
    while let Some(request) = stream.try_next().await? {
        let (id, responder) = request.into_on_pairing_complete().expect("only one method");
        let _ = responder.send();
        info!(id:% = PeerId::from(id); "Successful Fast Pair pairing");
    }
    info!("Provider service ended");
    Ok(())
}

async fn process_pairing_events(mut stream: PairingDelegateRequestStream) -> Result<(), Error> {
    while let Some(request) = stream.try_next().await? {
        match request {
            PairingDelegateRequest::OnPairingRequest {
                peer,
                method,
                displayed_passkey,
                responder,
            } => {
                info!("Received pairing request for peer: {:?} with method: {:?}", peer, method);
                // Accept all "normal" pairing requests.
                let _ = responder.send(true, displayed_passkey);
            }
            PairingDelegateRequest::OnPairingComplete { id, success, .. } => {
                info!(id:% = PeerId::from(id); "Normal pairing complete (success = {})", success);
            }
            _ => {}
        }
    }
    info!("Pairing Delegate service ended");
    Ok(())
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let fast_pair_svc = connect_to_protocol::<ProviderMarker>()
        .context("failed to connect to `fastpair.Provider` service")?;
    let pairing_svc = connect_to_protocol::<PairingMarker>()
        .context("failed to connect to `sys.Pairing` service")?;
    let (fastpair_client, fastpair_server) = create_request_stream::<ProviderWatcherMarker>();

    if let Err(e) = fast_pair_svc.enable(fastpair_client).await {
        warn!("Couldn't enable Fast Pair Provider service: {:?}", e);
        return Ok(());
    }
    info!("Enabled Fast Pair Provider");

    let (pairing_client, pairing_server) = create_request_stream::<PairingDelegateMarker>();
    if let Err(e) = pairing_svc.set_pairing_delegate(
        InputCapability::None,
        OutputCapability::None,
        pairing_client,
    ) {
        warn!("Couldn't enable Pairing service: {:?}", e);
        return Ok(());
    }
    info!("Enabled Pairing service");

    let mut pairing_fut = pin!(process_pairing_events(pairing_server).fuse());
    let mut fastpair_fut = pin!(process_provider_events(fastpair_server).fuse());

    select! {
        pairing_result = pairing_fut => info!("Pairing service unexpectedly finished: {:?}", pairing_result),
        fastpair_result = fastpair_fut => info!("Fast Pair service unexpectedly finished: {:?}", fastpair_result),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_test_helpers::run_while;
    use async_utils::PollExt;
    use fidl_fuchsia_bluetooth_sys::{PairingMethod, Peer};
    use fuchsia_async as fasync;
    use futures::pin_mut;

    #[fuchsia::test]
    fn process_pairing_requests() {
        let mut exec = fasync::TestExecutor::new();

        let (pairing_client, pairing_server) =
            fidl::endpoints::create_proxy_and_stream::<PairingDelegateMarker>();
        let pairing_fut = process_pairing_events(pairing_server);
        pin_mut!(pairing_fut);
        exec.run_until_stalled(&mut pairing_fut).expect_pending("server active");

        // Incoming pairing request - expect the server to receive it and reply positively.
        let passkey = 123456;
        let mut request_fut = pairing_client.on_pairing_request(
            &Peer::default(),
            PairingMethod::PasskeyComparison,
            passkey,
        );
        let (pairing_result, mut pairing_fut) =
            run_while(&mut exec, &mut pairing_fut, &mut request_fut);

        let (success, returned_passkey) = pairing_result.expect("successful FIDL request");
        assert!(success);
        assert_eq!(returned_passkey, passkey);

        // Incoming normal pairing complete event. Should be received, but no work to be done.
        let id = fidl_fuchsia_bluetooth::PeerId { value: 123 };
        pairing_client
            .on_pairing_complete(&id.into(), /* success*/ true)
            .expect("successful request");
        exec.run_until_stalled(&mut pairing_fut).expect_pending("server active");

        // Upstream disconnects.
        drop(pairing_client);
        let result = exec.run_until_stalled(&mut pairing_fut).expect("stream terminated");
        assert!(result.is_ok());
    }

    #[fuchsia::test]
    fn process_fastpair_events() {
        let mut exec = fasync::TestExecutor::new();

        let (provider_client, provider_server) =
            fidl::endpoints::create_proxy_and_stream::<ProviderWatcherMarker>();
        let provider_fut = process_provider_events(provider_server);
        pin_mut!(provider_fut);
        exec.run_until_stalled(&mut provider_fut).expect_pending("server active");

        let id = fidl_fuchsia_bluetooth::PeerId { value: 123 };
        let mut complete_fut = provider_client.on_pairing_complete(&id);
        let (result, mut provider_fut) = run_while(&mut exec, &mut provider_fut, &mut complete_fut);
        // Server should handle the request and respond.
        assert!(result.is_ok());

        // Upstream disconnects.
        drop(provider_client);
        let result = exec.run_until_stalled(&mut provider_fut).expect("stream terminated");
        assert!(result.is_ok());
    }
}
