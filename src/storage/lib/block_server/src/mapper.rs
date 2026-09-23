// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::verifier::Verifier;
use anyhow::Error;
use fidl_fuchsia_storage_block as fblock;
use fuchsia_async as fasync;
use futures::future::FutureExt as _;
use futures::stream::TryStreamExt as _;
use std::sync::Arc;

pub trait MapperHandler: Send + Sync + 'static {
    fn on_open_mapper_session(
        &self,
        mapping_vmo: &zx::Vmo,
        delivery_queue: zx::Vmo,
    ) -> Result<Verifier, zx::Status>;
}

impl<F> MapperHandler for F
where
    F: Fn(&zx::Vmo, zx::Vmo) -> Result<Verifier, zx::Status> + Send + Sync + 'static,
{
    fn on_open_mapper_session(
        &self,
        mapping_vmo: &zx::Vmo,
        delivery_queue: zx::Vmo,
    ) -> Result<Verifier, zx::Status> {
        self(mapping_vmo, delivery_queue)
    }
}

struct MapperVmoThread {
    mapping_vmo: zx::Vmo,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl MapperVmoThread {
    fn spawn<D: mapping::DeliveryHandler>(
        mapping_vmo: &zx::Vmo,
        files: Arc<mapping::Files<dyn mapping::reader::BlockService, D>>,
    ) -> Result<Self, Error> {
        let mapping_vmo_dup = mapping_vmo.duplicate_handle(zx::Rights::SAME_RIGHTS)?;
        let thread_vmo = mapping_vmo.duplicate_handle(zx::Rights::SAME_RIGHTS)?;
        let files_for_vmo = files.clone();
        let thread =
            std::thread::spawn(
                move || match vmo_fifo::Receiver::<mapping::RawMappingCommand>::new(
                    mapping_vmo_dup,
                    mapping::PENDING_COMMANDS_CAPACITY,
                ) {
                    Ok(mut receiver) => {
                        while let Ok(msg) = receiver.peek() {
                            if let Err(error) =
                                mapping::process_mapping_command(&msg, &files_for_vmo)
                            {
                                log::error!(error:?; "Failed to process mapping command");
                            }
                            if let Err(error) = msg.pop() {
                                log::error!(error:?; "Failed to pop mapping command from FIFO");
                            }
                        }
                    }
                    Err(error) => {
                        log::error!(error:?; "Failed to create mapping VMO FIFO receiver");
                    }
                },
            );
        Ok(Self { mapping_vmo: thread_vmo, thread: Some(thread) })
    }
}

impl Drop for MapperVmoThread {
    fn drop(&mut self) {
        let _ = self.mapping_vmo.signal(zx::Signals::empty(), vmo_fifo::SIG_SHUTDOWN);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

async fn run_mapper_session_loop<M: MapperHandler + ?Sized, D: mapping::DeliveryHandler>(
    handler: Arc<M>,
    service: Arc<dyn mapping::reader::BlockService>,
    session: fidl::endpoints::ServerEnd<fblock::MapperSessionMarker>,
    mapping_vmo: zx::Vmo,
    files: Arc<mapping::Files<dyn mapping::reader::BlockService, D>>,
) -> Result<(), Error> {
    let _mapper_vmo_thread = MapperVmoThread::spawn(&mapping_vmo, files.clone())?;

    let scope = fasync::Scope::new();
    let mut stream = session.into_stream();
    while let Some(request) = stream.try_next().await? {
        match request {
            fblock::MapperSessionRequest::OpenChildSession {
                session,
                mapping_vmo,
                parent_key,
                port,
                delivery_queue,
                responder,
            } => {
                let parent_file = match files.wait_for_file(parent_key).await {
                    Ok(file) => file,
                    Err(error) => {
                        log::warn!(error:?; "Failed to load parent file for key {parent_key}");
                        responder.send(Err(zx::Status::NOT_FOUND.into_raw()))?;
                        continue;
                    }
                };
                let child_service =
                    Arc::new(mapping::reader::ChildBlockService::new(service.clone(), parent_file));
                match serve_mapper_session(
                    handler.clone(),
                    child_service,
                    session,
                    mapping_vmo,
                    port,
                    delivery_queue,
                ) {
                    Ok(child_fut) => {
                        scope.spawn(async move {
                            if let Err(e) = child_fut.await {
                                log::warn!(e:?; "Child mapper session failed");
                            }
                        });
                        responder.send(Ok(()))?;
                    }
                    Err(status) => {
                        log::warn!(status:?; "serve_mapper_session failed for child session");
                        responder.send(Err(status.into_raw()))?;
                    }
                }
            }
            fblock::MapperSessionRequest::Close { responder } => {
                responder.send(Ok(()))?;
                break;
            }
            fblock::MapperSessionRequest::_UnknownMethod { .. } => {}
        }
    }

    scope.cancel().await;
    Ok(())
}

pub fn serve_mapper_session<M: MapperHandler + ?Sized>(
    handler: Arc<M>,
    service: Arc<dyn mapping::reader::BlockService>,
    session: fidl::endpoints::ServerEnd<fblock::MapperSessionMarker>,
    mapping_vmo: zx::Vmo,
    port: Option<zx::Port>,
    delivery_queue: Option<zx::Vmo>,
) -> Result<futures::future::BoxFuture<'static, Result<(), Error>>, zx::Status> {
    match (port, delivery_queue) {
        (Some(port), Some(delivery_queue)) => {
            let verifier = handler.on_open_mapper_session(&mapping_vmo, delivery_queue)?;
            let files = Arc::new(mapping::Files::new(service.clone(), verifier));
            let pager_thread = mapping::PagerThread::spawn(port, files.clone());
            Ok(async move {
                let _pager_thread = pager_thread;
                run_mapper_session_loop(handler, service, session, mapping_vmo, files).await
            }
            .boxed())
        }
        (None, None) => {
            let files = Arc::new(mapping::Files::new_without_pager(service.clone()));
            Ok(async move {
                run_mapper_session_loop(handler, service, session, mapping_vmo, files).await
            }
            .boxed())
        }
        _ => Err(zx::Status::INVALID_ARGS),
    }
}
