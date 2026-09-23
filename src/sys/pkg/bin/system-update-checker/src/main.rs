// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(clippy::let_unit_value)]
#![allow(clippy::type_complexity)]

mod apply;
mod channel;
mod channel_handler;
mod check;
mod completion_responder;
mod connect;
mod errors;
mod rate_limiter;
mod update_manager;
mod update_monitor;
mod update_service;

use crate::channel_handler::ChannelHandler;
use crate::completion_responder::{CompletionResponder, CompletionResponderFidlServer};
use crate::update_service::{RealUpdateManager, UpdateService};
use anyhow::{Context as _, Error, anyhow};
use fidl_fuchsia_update::ListenerRequestStream;
use fidl_fuchsia_update_channel::ProviderRequestStream;
use fidl_fuchsia_update_channelcontrol::ChannelControlRequestStream;
use fuchsia_component::server::ServiceFs;
use fuchsia_inspect as finspect;
use futures::prelude::*;
use futures::stream::FuturesUnordered;
use log::error;
use std::sync::Arc;
use system_update_checker_config::Config;

const MAX_CONCURRENT_CONNECTIONS: usize = 200;
const DEFAULT_UPDATE_PACKAGE_URL: &str = "fuchsia-pkg://fuchsia.com/update";

#[fuchsia::main(logging_tags = ["system-update-checker"])]
async fn main() -> Result<(), Error> {
    main_inner().await.map_err(|err| {
        // anyhow with alternate formatting prints the Display impl of each error in the chain.
        let err = anyhow!(err);
        error!("error running system-update-checker: {:#}", err);
        err
    })
}

async fn main_inner() -> Result<(), Error> {
    let config = Config::take_from_startup_handle();
    let inspector = finspect::Inspector::default();
    let _inspect_server_task =
        inspect_runtime::publish(&inspector, inspect_runtime::PublishOptions::default());

    let target_channel_manager = channel::TargetChannelManager::new(
        connect::ServiceConnector,
        "/config/data",
        config.current_channel.clone(),
    );
    target_channel_manager.update();
    let target_channel_manager = Arc::new(target_channel_manager);

    let futures = FuturesUnordered::new();

    let current_channel_manager = channel::CurrentChannelManager::new(config.current_channel);
    let current_channel_manager = Arc::new(current_channel_manager);

    let (completion_responder_state_reactor, completion_responder_fidl_server, waiter) =
        CompletionResponder::build();
    futures.push(waiter.wait_for_messages().boxed());
    let (update_manager, update_manager_fut) = RealUpdateManager::new(
        Arc::clone(&target_channel_manager),
        completion_responder_state_reactor,
        inspector.root().create_child("update-manager"),
    )
    .await
    .start();
    futures.push(update_manager_fut.boxed());

    let mut fs = ServiceFs::new();
    let update_manager_clone = update_manager.clone();
    let channel_handler =
        Arc::new(ChannelHandler::new(current_channel_manager, target_channel_manager));
    let channel_handler_clone = Arc::clone(&channel_handler);
    let channel_handler_provider_clone = Arc::clone(&channel_handler);
    let completion_responder_fidl_server_clone = completion_responder_fidl_server.clone();
    fs.dir("svc")
        .add_fidl_service(move |stream| {
            IncomingServices::Manager(stream, UpdateService::new(update_manager_clone.clone()))
        })
        .add_fidl_service(move |stream| {
            IncomingServices::Provider(stream, Arc::clone(&channel_handler_provider_clone))
        })
        .add_fidl_service(move |stream| {
            IncomingServices::ChannelControl(stream, Arc::clone(&channel_handler_clone))
        })
        .add_fidl_service(move |stream| {
            IncomingServices::CompletionResponder(
                stream,
                completion_responder_fidl_server_clone.clone(),
            )
        });

    fs.take_and_serve_directory_handle().context("ServiceFs::take_and_serve_directory_handle")?;
    futures.push(
        fs.for_each_concurrent(MAX_CONCURRENT_CONNECTIONS, |incoming_service| {
            handle_incoming_service(incoming_service)
                .unwrap_or_else(|e| error!("error handling client connection: {:#}", anyhow!(e)))
        })
        .boxed(),
    );

    futures.collect::<()>().await;

    Ok(())
}

enum IncomingServices {
    Manager(fidl_fuchsia_update::ManagerRequestStream, UpdateService),
    Provider(ProviderRequestStream, Arc<ChannelHandler>),
    ChannelControl(ChannelControlRequestStream, Arc<ChannelHandler>),
    CompletionResponder(ListenerRequestStream, CompletionResponderFidlServer),
}

async fn handle_incoming_service(incoming_service: IncomingServices) -> Result<(), Error> {
    match incoming_service {
        IncomingServices::Manager(request_stream, mut update_service) => {
            update_service.handle_request_stream(request_stream).await
        }
        IncomingServices::Provider(request_stream, handler) => {
            handler.handle_provider_request_stream(request_stream).await
        }
        IncomingServices::ChannelControl(request_stream, handler) => {
            handler.handle_control_request_stream(request_stream).await
        }
        IncomingServices::CompletionResponder(request_stream, fidl_server) => {
            fidl_server.serve_completion_responses(request_stream).await
        }
    }
}
