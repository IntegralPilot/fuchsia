// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The Archivist collects and stores diagnostic data from components.

#![warn(missing_docs)]

use anyhow::{Context, Error};
use archivist_config::Config;
use archivist_lib::archivist::Archivist;
use archivist_lib::component_lifecycle;
use archivist_lib::logs::repository::{ARCHIVIST_MONIKER, ARCHIVIST_TAG};
use archivist_lib::logs::serial::MAX_SERIAL_WRITE_SIZE;
use archivist_lib::logs::shared_buffer::create_ring_buffer;
use diagnostics_log_encoding::encode::{
    Encoder, EncoderOpts, LogEvent, MutableBuffer, WriteEventParams,
};
use fidl_fuchsia_logger::MAX_DATAGRAM_LEN_BYTES;
use fuchsia_async::SendExecutorBuilder;
use fuchsia_component::client;
use fuchsia_component::server::{MissingStartupHandle, ServiceFs};
use fuchsia_inspect::component;
use fuchsia_inspect::health::Reporter;
use log::{LevelFilter, debug};
use moniker::Moniker;
use std::fmt;
use std::io::Cursor;
use std::sync::atomic::{AtomicU64, Ordering};
use {fidl_fuchsia_component_sandbox as fsandbox, fuchsia_async as fasync, fuchsia_runtime as rt};

const INSPECTOR_SIZE: usize = 2 * 1024 * 1024 /* 2MB */;

fn main() -> Result<(), Error> {
    let config = Config::take_from_startup_handle();
    // NOTE: Sockets are serviced from a dedicated thread (which isn't counted here).
    if config.num_threads == 1 {
        fasync::LocalExecutor::default().run_singlethreaded(async_main(config))
    } else {
        // The executor will spin up an extra thread which is only for monitoring, so we ignore
        // that.
        let mut executor = SendExecutorBuilder::new().num_threads(config.num_threads).build();
        executor.run(async_main(config))
    }
    .context("async main")?;
    debug!("Exiting.");
    Ok(())
}

fn write_to_serial(msg: &str) {
    let msg = msg.as_bytes();
    for chunk in msg.chunks(MAX_SERIAL_WRITE_SIZE) {
        unsafe {
            zx::sys::zx_debug_write(chunk.as_ptr(), chunk.len());
        }
    }
}

async fn async_main(config: Config) -> Result<(), Error> {
    let ring_buffer = create_ring_buffer(config.logs_max_cached_original_bytes as usize);

    // We assume that this is the embedded version of Archivist if enable_klog is false.
    let is_embedded = !config.enable_klog;

    if let Err(e) = init_diagnostics(&ring_buffer, is_embedded).await {
        write_to_serial(&format!("archivist: init_diagnostics failed: {e:?}\n"));
        return Err(e).context("init_diagnostics");
    }

    component::inspector()
        .root()
        .record_child("config", |config_node| config.record_inspect(config_node));

    let archivist = Archivist::new(ring_buffer, config).await;
    debug!("Archivist initialized from configuration.");

    let startup_handle =
        fuchsia_runtime::take_startup_handle(fuchsia_runtime::HandleType::DirectoryRequest.into())
            .ok_or(MissingStartupHandle)?;

    let mut fs = ServiceFs::new();
    fs.serve_connection(fidl::endpoints::ServerEnd::new(zx::Channel::from(startup_handle)))?;
    let component_store = client::connect_to_protocol::<fsandbox::CapabilityStoreMarker>()
        .context("connect to factory")?;
    let lifecycle_requests = component_lifecycle::take_lifecycle_request_stream();
    archivist.run(fs, is_embedded, component_store, lifecycle_requests).await?;

    Ok(())
}

async fn init_diagnostics(
    ring_buffer: &ring_buffer::Reader,
    is_embedded: bool,
) -> Result<(), Error> {
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        write_to_serial(&format!("[archivist] FATAL: {info}\n"));
        previous_hook(info);
    }));

    struct Logger {
        iob: zx::Iob,
        dropped: AtomicU64,
    }

    // Used for redirecting stdout and stderr.
    struct Redirector {
        severity: log::Level,
    }

    impl fdio_zxio::FdOps for Redirector {
        fn writev(&self, iovecs: &[zx::sys::zx_iovec_t]) -> Result<usize, zx::Status> {
            struct Iovecs<'a>(&'a [zx::sys::zx_iovec_t]);

            impl fmt::Display for Iovecs<'_> {
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    let mut iter = self.0.iter();
                    let last = iter.next_back();
                    for vec in iter {
                        f.write_str(&String::from_utf8_lossy(unsafe {
                            std::slice::from_raw_parts(vec.buffer, vec.capacity)
                        }))?;
                    }
                    // Strip the trailing \n if there is one.
                    if let Some(last) = last {
                        let amount = if last.capacity > 0
                            && unsafe { *last.buffer.add(last.capacity - 1) } == b'\n'
                        {
                            last.capacity - 1
                        } else {
                            last.capacity
                        };
                        f.write_str(&String::from_utf8_lossy(unsafe {
                            std::slice::from_raw_parts(last.buffer, amount)
                        }))?;
                    }
                    Ok(())
                }
            }

            log::log!(self.severity, "{}", Iovecs(iovecs));

            Ok(iovecs.iter().map(|i| i.capacity).sum())
        }

        fn isatty(&self) -> Result<bool, zx::Status> {
            Ok(true)
        }
    }

    thread_local! {
        static PID_AND_TID: (zx::Koid, zx::Koid) = (
            rt::process_self()
                .koid()
                .unwrap_or_else(|_| zx::Koid::from_raw(zx::sys::zx_koid_t::MAX)),
            rt::with_thread_self(|thread| {
                thread.koid().unwrap_or_else(|_| zx::Koid::from_raw(zx::sys::zx_koid_t::MAX))
            }),
        );
    }

    impl log::Log for Logger {
        fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
            // NOTE: we handle minimum severity directly through the log max_level. So we call,
            // log::set_max_level, log::max_level where appropriate.
            true
        }

        fn log(&self, record: &log::Record<'_>) {
            let mut buf = [0u8; MAX_DATAGRAM_LEN_BYTES as _];
            let mut encoder = Encoder::new(Cursor::new(&mut buf[..]), EncoderOpts::default());
            let (pid, tid) = PID_AND_TID.with(|r| *r);
            let tags: &[&str] = &[];
            let dropped = self.dropped.swap(0, Ordering::Relaxed);
            if encoder
                .write_event(WriteEventParams {
                    event: LogEvent::new(record),
                    tags,
                    metatags: std::iter::empty(),
                    pid,
                    tid,
                    dropped,
                })
                .is_err()
            {
                self.dropped.fetch_add(dropped + 1, Ordering::Relaxed);
                return;
            }
            let end = encoder.inner().cursor();
            if self.iob.write(Default::default(), 0, &encoder.inner().get_ref()[..end]).is_err() {
                self.dropped.fetch_add(dropped + 1, Ordering::Relaxed);
            }
        }

        fn flush(&self) {}
    }

    // To set up logging for Archivist, we need to create the ring buffer early. Later, we will
    // create a container for it with this moniker.
    ARCHIVIST_MONIKER
        .set(if is_embedded {
            Moniker::parse_str("archivist").unwrap()
        } else {
            Moniker::parse_str("bootstrap/archivist").unwrap()
        })
        .unwrap();

    log::set_max_level(LevelFilter::Info);
    log::set_boxed_logger(Box::new(Logger {
        iob: ring_buffer.new_iob_writer(ARCHIVIST_TAG)?.0,
        dropped: AtomicU64::new(0),
    }))?;

    // NOTE: For the embedded archivist, we use the Debug level because there are integration tests
    // that will fail if they spuriously see log messages from Archivist. As an example, the
    // fuchsia-async library can write to stderr when tasks appear to be stalled, and that can
    // happen on slow builders (through no fault of Archivist). If that happens, then it can cause
    // some integration tests that are particular about what log messages are produced to fail.
    fdio_zxio::bind_to_fd_with_ops(
        Redirector { severity: if is_embedded { log::Level::Debug } else { log::Level::Info } },
        1,
    )?;
    fdio_zxio::bind_to_fd_with_ops(
        Redirector { severity: if is_embedded { log::Level::Debug } else { log::Level::Warn } },
        2,
    )?;

    component::init_inspector_with_size(INSPECTOR_SIZE);
    component::health().set_starting_up();

    fuchsia_trace_provider::trace_provider_create_with_fdio();
    Ok(())
}
