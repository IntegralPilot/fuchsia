// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::sync::Arc;

use anyhow::{Context, Error};
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::LocalComponentHandles;
use futures::lock::Mutex;
use futures::{StreamExt, TryStreamExt};
use log::{error, info};
use {fidl_fuchsia_hardware_google_nanohub as fuchsia_nanohub, fuchsia_async};

pub struct FakeDataChannel {
    stat_flags: Arc<Mutex<u32>>,
    event: Arc<Mutex<Option<zx::Event>>>,
    channel_name: String,
    read_data: String,
}

enum Incoming {
    StarnixDataChannelService(fuchsia_nanohub::StarnixDataChannelServiceRequest),
}

impl FakeDataChannel {
    pub fn new(channel_name: String, read_data: String) -> Self {
        FakeDataChannel {
            stat_flags: Arc::new(Mutex::new(fuchsia_nanohub::SIGNAL_WRITABLE)),
            event: Arc::new(Mutex::new(None)),
            channel_name,
            read_data,
        }
    }

    async fn handle_unbound_waitable_stream(
        self: Arc<Self>,
        mut stream: fuchsia_nanohub::UnboundWaitableDataChannelRequestStream,
    ) -> Result<(), Error> {
        while let Some(request) = stream.try_next().await.context("failed request")? {
            match request {
                fuchsia_nanohub::UnboundWaitableDataChannelRequest::GetIdentifier { responder } => {
                    let response =
                        fuchsia_nanohub::UnboundWaitableDataChannelGetIdentifierResponse {
                            name: Some(self.channel_name.clone()),
                            ..Default::default()
                        };

                    let _ = responder.send(&response);
                }
                fuchsia_nanohub::UnboundWaitableDataChannelRequest::Bind { payload, responder } => {
                    let stream = payload
                        .server
                        .expect("Server end of waitableDataChannel not provided")
                        .into_stream();

                    let event = payload.event.expect("Event not provided");
                    if let Err(e) = event.signal(
                        zx::Signals::empty(),
                        zx::Signals::from_bits_truncate(*self.stat_flags.lock().await),
                    ) {
                        info!("Failed to signal event: {:?}", e);
                    }
                    let mut event_lock = self.event.lock().await;
                    *event_lock = Some(event);

                    let self_clone = self.clone();
                    fuchsia_async::Task::spawn(async move {
                        if let Err(e) = self_clone.handle_waitable_stream(stream).await {
                            error!("Error handling bound waitable device stream: {:?}", e);
                        }
                    })
                    .detach();
                    let _ = responder.send(Ok(()));
                }
                _ => {
                    error!("unexpected waitable request");
                }
            }
        }
        Ok(())
    }

    async fn handle_waitable_stream(
        &self,
        mut stream: fuchsia_nanohub::WaitableDataChannelRequestStream,
    ) -> Result<(), Error> {
        while let Some(request) = stream.try_next().await.context("failed request")? {
            match request {
                fuchsia_nanohub::WaitableDataChannelRequest::Read { responder } => {
                    let response = fuchsia_nanohub::WaitableDataChannelReadResponse {
                        data: Some(self.read_data.as_bytes().to_vec()),
                        ..Default::default()
                    };
                    let _ = responder.send(Ok(response));
                }
                fuchsia_nanohub::WaitableDataChannelRequest::Write { payload: _, responder } => {
                    let mut stat_flags = self.stat_flags.lock().await;
                    *stat_flags |= fuchsia_nanohub::SIGNAL_READABLE;
                    if let Some(event) = &*self.event.lock().await {
                        if let Err(e) = event.signal(
                            zx::Signals::empty(),
                            zx::Signals::from_bits_truncate(*stat_flags),
                        ) {
                            info!("failed to signal event: {:?}", e);
                        }
                    }
                    let _ = responder.send(Ok(()));
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub async fn fake_driverservice(
        channel_name: String,
        read_data: String,
        handles: LocalComponentHandles,
    ) -> Result<(), Error> {
        let fake = Arc::new(Self::new(channel_name, read_data));
        let mut fs = ServiceFs::new();

        fs.dir("svc").add_fidl_service_instance("default", Incoming::StarnixDataChannelService);

        fs.serve_connection(handles.outgoing_dir)?;

        fs.for_each_concurrent(0, move |request| {
            let fake = fake.clone();
            async move {
                match request {
                    Incoming::StarnixDataChannelService(
                        fuchsia_nanohub::StarnixDataChannelServiceRequest::Waitable(stream),
                    ) => {
                        if let Err(e) = fake.handle_unbound_waitable_stream(stream).await {
                            error!("Error handling waitable stream: {:?}", e);
                        }
                    }
                    _ => {
                        error!("Unexpected request");
                    }
                }
            }
        })
        .await;

        Ok(())
    }
}
