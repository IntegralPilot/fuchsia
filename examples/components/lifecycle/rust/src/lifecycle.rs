// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::anyhow;
use fidl::endpoints::ServerEnd;
use fidl::handle::AsyncChannel;
use fidl::prelude::*;
use fuchsia_runtime::{HandleInfo, HandleType};
use futures_util::stream::TryStreamExt;
use log::{error, info};
use std::process;
use {fidl_fuchsia_component_sandbox as fsandbox, fidl_fuchsia_process_lifecycle as flifecycle};

// [START imports]
use fidl_fuchsia_process_lifecycle::{LifecycleRequest, LifecycleRequestStream};
// [END imports]

#[allow(dead_code)]
fn to_err(sandbox: fsandbox::CapabilityStoreError) -> anyhow::Error {
    anyhow!("{:#?}", sandbox)
}

/// This function is unused because we are only using it for documentation, so we just need to know
/// that it compiles.
async fn _escrow_example() -> Result<(), anyhow::Error> {
    // [START escrow_listening]
    let lifecycle =
        fuchsia_runtime::take_startup_handle(HandleInfo::new(HandleType::Lifecycle, 0)).unwrap();
    let lifecycle: zx::Channel = lifecycle.into();
    let lifecycle: ServerEnd<flifecycle::LifecycleMarker> = lifecycle.into();
    let (lifecycle_request_stream, lifecycle_control_handle) =
        lifecycle.into_stream_and_control_handle();

    let outgoing_dir = None;
    // Later, when `ServiceFs` has stalled and we have an `outgoing_dir`.
    lifecycle_control_handle
        .send_on_escrow(flifecycle::LifecycleOnEscrowRequest { outgoing_dir, ..Default::default() })
        .unwrap();
    // [END escrow_listening]

    let _ = lifecycle_request_stream;

    // [START escrow_create_dictionary]
    let capability_store = fuchsia_component::client::connect_to_protocol::<
        fidl_fuchsia_component_sandbox::CapabilityStoreMarker,
    >()
    .unwrap();
    let id_generator = sandbox::CapabilityIdGenerator::new();
    let dictionary_id = id_generator.next();
    capability_store.dictionary_create(dictionary_id).await?.map_err(to_err)?;
    // [END escrow_create_dictionary]

    let outgoing_dir = None;

    // [START escrow_populate_dictionary]
    let bytes = vec![1, 2, 3];
    let data_id = id_generator.next();
    capability_store
        .import(data_id, fsandbox::Capability::Data(fsandbox::Data::Bytes(bytes)))
        .await?
        .map_err(to_err)?;
    capability_store
        .dictionary_insert(
            dictionary_id,
            &fsandbox::DictionaryItem { key: "my_data".to_string(), value: data_id },
        )
        .await?
        .map_err(to_err)?;
    let fsandbox::Capability::Dictionary(dictionary_ref) =
        capability_store.export(dictionary_id).await?.map_err(to_err)?
    else {
        panic!("Bad export");
    };
    // [END escrow_populate_dictionary]

    // [START escrow_send_dictionary]
    lifecycle_control_handle.send_on_escrow(flifecycle::LifecycleOnEscrowRequest {
        outgoing_dir: outgoing_dir,
        escrowed_dictionary: Some(dictionary_ref),
        ..Default::default()
    })?;
    // [END escrow_send_dictionary]

    // [START escrow_receive_dictionary]
    let Some(dictionary) =
        fuchsia_runtime::take_startup_handle(HandleInfo::new(HandleType::EscrowedDictionary, 0))
    else {
        return Err(anyhow!("Couldn't find startup handle"));
    };

    let dict_id = id_generator.next();
    capability_store
        .import(
            dict_id,
            fsandbox::Capability::Dictionary(fsandbox::DictionaryRef { token: dictionary.into() }),
        )
        .await?
        .map_err(to_err)?;

    let capability_id = id_generator.next();
    capability_store
        .dictionary_remove(
            dict_id,
            "my_data",
            Some(&fsandbox::WrappedNewCapabilityId { id: capability_id }),
        )
        .await?
        .map_err(to_err)?;
    let fsandbox::Capability::Data(data) =
        capability_store.export(capability_id).await?.map_err(to_err)?
    else {
        return Err(anyhow!("Bad capability type from dictionary"));
    };
    // Do something with the data...
    // [END escrow_receive_dictionary]

    let _ = data;
    Ok(())
}

// [START lifecycle_handler]
#[fuchsia::main(logging_tags = ["lifecycle", "example"])]
async fn main() {
    // Take the lifecycle handle provided by the runner
    match fuchsia_runtime::take_startup_handle(HandleInfo::new(HandleType::Lifecycle, 0)) {
        Some(lifecycle_handle) => {
            info!("Lifecycle channel received.");
            // Begin listening for lifecycle requests on this channel
            let x: zx::Channel = lifecycle_handle.into();
            let async_x = AsyncChannel::from(fuchsia_async::Channel::from_channel(x));
            let mut req_stream = LifecycleRequestStream::from_channel(async_x);
            info!("Awaiting request to close...");
            if let Some(request) =
                req_stream.try_next().await.expect("Failure receiving lifecycle FIDL message")
            {
                match request {
                    LifecycleRequest::Stop { control_handle: c } => {
                        info!("Received request to stop. Shutting down.");
                        c.shutdown();
                        process::exit(0);
                    }
                }
            }

            // We only arrive here if the lifecycle channel closed without
            // first sending the shutdown event, which is unexpected.
            process::abort();
        }
        None => {
            // We did not receive a lifecycle channel, exit abnormally.
            error!("No lifecycle channel received, exiting.");
            process::abort();
        }
    }
}
// [END lifecycle_handler]
