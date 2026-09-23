// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod fifo;

use anyhow::{Context, Error};
use fidl::endpoints::DiscoverableProtocolMarker;
use fidl_fuchsia_hardware_pty::{self as pty, WindowSize};
use fidl_fuchsia_kernel as fkernel;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use futures::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

// This should be std::ascii::Char::EndOfText once stabilized.
const END_OF_TEXT: u8 = 3;

#[fuchsia::main(logging = false)]
async fn main() -> Result<(), Error> {
    stdout_to_debuglog::init().await?;
    println!("console: starting up");

    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(|stream: pty::DeviceRequestStream| stream);
    fs.take_and_serve_directory_handle()?;

    let (event1, event2) = zx::EventPair::create();
    let fifo = Arc::new(fifo::Fifo::new(event1));
    let console_service = Arc::new(ConsoleService::new(fifo.clone(), event2));

    let resource = get_debug_resource().await.context("Could not get debug resource")?;
    let thread_console_service = console_service.clone();
    std::thread::spawn(move || {
        debug_reader_thread(resource, fifo, thread_console_service);
    });

    while let Some(stream) = fs.next().await {
        console_service.serve_pty(stream);
    }

    println!("console exiting");

    Ok(())
}

async fn get_debug_resource() -> Result<zx::Resource, Error> {
    let proxy = fuchsia_component::client::connect_to_protocol::<fkernel::DebugResourceMarker>()?;
    let resource = proxy.get().await?;
    Ok(resource)
}

fn debug_reader_thread(
    resource: zx::Resource,
    fifo: Arc<fifo::Fifo>,
    console_service: Arc<ConsoleService>,
) {
    let mut logged_write_failure = false;

    loop {
        let mut buf = [0u8; 16];
        let mut actual: usize = 0;
        let result = unsafe {
            zx::ok(zx::sys::zx_debug_read(
                resource.raw_handle(),
                buf.as_mut_ptr(),
                buf.len(),
                &raw mut actual,
            ))
        };
        match result {
            Ok(()) => {
                for i in 0..actual {
                    let ch = buf[i];
                    let features = console_service.features.load(Ordering::Relaxed);
                    if features & pty::FEATURE_RAW == 0 && ch == END_OF_TEXT {
                        // CTRL-C
                        console_service
                            .event_mask
                            .fetch_or(pty::EVENT_INTERRUPT, Ordering::Relaxed);
                        console_service
                            .rx_event
                            .signal(zx::Signals::NONE, zx::Signals::USER_1)
                            .unwrap();
                        continue;
                    }
                    if let Err(e) = fifo.write(&buf[i..=i])
                        && !logged_write_failure
                    {
                        println!(
                            "console: failed to write to fifo: {}, silencing future failures",
                            e
                        );
                        logged_write_failure = true;
                    }
                }
            }
            Err(zx::Status::NOT_SUPPORTED) => {
                // No console on this machine.
                return;
            }
            Err(e) => {
                println!("console: zx_debug_read failed: {}, exiting.", e);
                return;
            }
        }
    }
}

struct ConsoleService {
    fifo: Arc<fifo::Fifo>,
    rx_event: zx::EventPair,
    features: AtomicU32,
    event_mask: AtomicU32,
    scope: fasync::Scope,
}

impl ConsoleService {
    fn new(fifo: Arc<fifo::Fifo>, rx_event: zx::EventPair) -> Self {
        Self {
            fifo,
            rx_event,
            features: AtomicU32::new(0),
            event_mask: AtomicU32::new(0),
            scope: fasync::Scope::new_with_name("console service"),
        }
    }

    fn serve_pty(self: &Arc<Self>, stream: pty::DeviceRequestStream) {
        let service = self.clone();
        self.scope.spawn_local(async move {
            if let Err(e) = service.serve_pty_inner(stream).await {
                println!("console: failed to serve pty: {}", e);
            }
        });
    }

    async fn serve_pty_inner(
        self: Arc<Self>,
        mut stream: pty::DeviceRequestStream,
    ) -> Result<(), fidl::Error> {
        while let Some(request) = stream.try_next().await? {
            match request {
                pty::DeviceRequest::Read { count, responder } => {
                    match self.fifo.read(count as usize) {
                        Ok(buf) => {
                            responder.send(Ok(&buf[..]))?;
                        }
                        Err(status) => {
                            responder.send(Err(status.into_raw()))?;
                        }
                    }
                }
                pty::DeviceRequest::Write { data, responder } => {
                    let mut offset = 0;
                    loop {
                        if offset >= data.len() {
                            responder.send(Ok(data.len() as u64))?;
                            break;
                        }
                        let write_len = std::cmp::min(data.len() - offset, 256);
                        let slice = &data[offset..offset + write_len];
                        if let Err(e) =
                            unsafe { zx::ok(zx::sys::zx_debug_write(slice.as_ptr(), slice.len())) }
                        {
                            responder.send(Err(e.into_raw()))?;
                            break;
                        }
                        offset += write_len;
                    }
                }
                pty::DeviceRequest::Describe { responder } => {
                    let event = self
                        .rx_event
                        .duplicate_handle(zx::Rights::SAME_RIGHTS)
                        .expect("handle should be duplicatable");
                    let info =
                        pty::DeviceDescribeResponse { event: Some(event), ..Default::default() };
                    responder.send(info)?;
                }
                pty::DeviceRequest::ClrSetFeature { clr, set, responder } => {
                    // Using relaxed atomics is correct here because we are using a single
                    // threaded executor and the debug reader thread never touches features.
                    let mut features = self.features.load(Ordering::Relaxed);
                    features |= set;
                    features &= !clr;
                    self.features.store(features, Ordering::Relaxed);
                    responder.send(zx::sys::ZX_OK, features)?;
                }
                pty::DeviceRequest::ReadEvents { responder } => {
                    let mask = self.event_mask.swap(0, Ordering::Relaxed);
                    self.rx_event
                        .signal(zx::Signals::USER_1, zx::Signals::NONE)
                        .expect("event should be signalable");
                    responder.send(zx::sys::ZX_OK, mask)?;
                }
                pty::DeviceRequest::GetWindowSize { responder } => {
                    let window_size = WindowSize { width: 0, height: 0 };
                    responder.send(zx::Status::NOT_SUPPORTED.into_raw(), &window_size)?;
                }
                pty::DeviceRequest::MakeActive { responder, .. } => {
                    responder.send(zx::Status::NOT_SUPPORTED.into_raw())?;
                }
                pty::DeviceRequest::SetWindowSize { responder, .. } => {
                    responder.send(zx::Status::NOT_SUPPORTED.into_raw())?;
                }
                pty::DeviceRequest::Close { responder } => {
                    responder.send(Ok(()))?;
                    // connection will be closed
                }
                pty::DeviceRequest::Clone { request, .. } => {
                    let request = fidl::endpoints::ServerEnd::<pty::DeviceMarker>::new(
                        request.into_channel(),
                    );
                    self.serve_pty(request.into_stream());
                }
                pty::DeviceRequest::Query { responder } => {
                    responder.send(pty::DeviceMarker::PROTOCOL_NAME.as_bytes())?;
                }
                pty::DeviceRequest::OpenClient { responder, .. } => {
                    responder.send(zx::Status::NOT_SUPPORTED.into_raw())?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_test() -> (Arc<ConsoleService>, pty::DeviceProxy) {
        let (ev1, ev2) = zx::EventPair::create();
        let fifo = Arc::new(fifo::Fifo::new(ev1));
        let service = Arc::new(ConsoleService::new(fifo, ev2));
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<pty::DeviceMarker>();
        service.serve_pty(stream);
        (service, proxy)
    }

    #[fuchsia::test]
    async fn read() {
        let (service, proxy) = setup_test();
        service.fifo.write(&[1, 2, 3]).unwrap();
        let data = proxy.read(3).await.unwrap().unwrap();
        assert_eq!(data, vec![1, 2, 3]);
    }

    #[fuchsia::test]
    async fn read_raw() {
        let (service, proxy) = setup_test();
        service.features.store(pty::FEATURE_RAW, Ordering::Relaxed);
        service.fifo.write(&[3]).unwrap();
        let data = proxy.read(1).await.unwrap().unwrap();
        assert_eq!(data, vec![3]);
    }

    #[fuchsia::test]
    async fn feature_bits() {
        let (_service, proxy) = setup_test();
        let (status, features) = proxy.clr_set_feature(0, 3).await.unwrap();
        zx::ok(status).unwrap();
        assert_eq!(features, 3);
        let (status, features) = proxy.clr_set_feature(1, 0).await.unwrap();
        zx::ok(status).unwrap();
        assert_eq!(features, 2);
    }

    #[fuchsia::test]
    async fn write() {
        let (_service, proxy) = setup_test();
        let result = proxy.write(&[1, 2, 3]).await.unwrap().unwrap();
        assert_eq!(result, 3);
    }
}
