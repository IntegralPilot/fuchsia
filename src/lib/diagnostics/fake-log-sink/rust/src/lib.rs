// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use diagnostics_message::MonikerWithUrl;
use fidl::endpoints::{RequestStream, ServerEnd};
use fidl_fuchsia_diagnostics_types::{Interest, Severity};
use fidl_fuchsia_logger::{
    LogSinkMarker, LogSinkOnInitRequest, LogSinkRequest, LogSinkWaitForInterestChangeResponder,
};
use fuchsia_async::{self as fasync, EHandle};
use fuchsia_sync::{Condvar, Mutex};
use futures::StreamExt;
use futures::future::{AbortHandle, Abortable};
use ring_buffer::RingBuffer;
use std::sync::Arc;

/// FakeLogSink serves LogSink connections and forward any messages logged to it.
pub struct FakeLogSink {
    ring_buffer: ring_buffer::Reader,
    min_severity: Arc<Mutex<MinSeverity>>,
    ehandle: EHandle,
    // This must be dropped after the ring buffer to avoid the executor complaining about
    // receivers outliving their executor.
    _abort_handle: DropAbortHandle,
}

struct DropAbortHandle(AbortHandle);

impl Drop for DropAbortHandle {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct MinSeverity {
    severity: Severity,
    updates: MinSeverityUpdates,
}

#[derive(Default)]
enum MinSeverityUpdates {
    #[default]
    None,
    Pending,
    Listeners(Vec<LogSinkWaitForInterestChangeResponder>),
}

impl Default for FakeLogSink {
    fn default() -> Self {
        FakeLogSink::new()
    }
}

impl FakeLogSink {
    /// Returns a new FakeLogSink and receiver that will have messages delivered to it.
    pub fn new() -> Self {
        Self::new_impl(false)
    }

    fn new_impl(sync: bool) -> Self {
        let (abort_handle, abort_registration) = AbortHandle::new_pair();
        let (ehandle, ring_buffer) = if sync {
            let ehandle_and_ring_buffer = Arc::new((Mutex::new(None), Condvar::new()));
            let eh_and_rb = ehandle_and_ring_buffer.clone();
            std::thread::spawn(|| {
                let _ = fasync::LocalExecutor::default().run_singlethreaded(Abortable::new(
                    async move {
                        let reader = RingBuffer::create(ring_buffer::MAX_MESSAGE_SIZE);
                        *eh_and_rb.0.lock() = Some((EHandle::local(), reader));
                        eh_and_rb.1.notify_all();
                        let () = std::future::pending().await;
                    },
                    abort_registration,
                ));
            });
            let mut ehandle_buffer_guard = ehandle_and_ring_buffer.0.lock();
            ehandle_and_ring_buffer.1.wait_while(&mut ehandle_buffer_guard, |rb| rb.is_none());
            ehandle_buffer_guard.take().unwrap()
        } else {
            let reader = RingBuffer::create(ring_buffer::MAX_MESSAGE_SIZE);
            fasync::Task::spawn(async {
                let _ = Abortable::new(std::future::pending::<()>(), abort_registration).await;
            })
            .detach();
            (EHandle::local(), reader)
        };

        Self {
            ring_buffer,
            min_severity: Arc::new(Mutex::new(MinSeverity {
                severity: Severity::Info,
                updates: MinSeverityUpdates::default(),
            })),
            ehandle,
            _abort_handle: DropAbortHandle(abort_handle),
        }
    }

    /// Returns a message sent to the sink.
    pub async fn read_message(&mut self) -> String {
        let (_tag, bytes) = self.ring_buffer.read_message().await.unwrap();
        diagnostics_message::from_structured(
            MonikerWithUrl { url: "".into(), moniker: "fake-log-sink".try_into().unwrap() },
            &bytes,
        )
        .unwrap()
        .msg()
        .unwrap()
        .into()
    }

    /// Handles the server end of the LogSink connection.
    pub fn serve(&self, server_end: ServerEnd<LogSinkMarker>) {
        let (iob, _) = self.ring_buffer.new_iob_writer(1).unwrap();
        let min_severity = self.min_severity.clone();
        self.ehandle.spawn_detached(async move {
            let mut requests = server_end.into_stream();
            {
                let mut min_severity = min_severity.lock();
                requests
                    .control_handle()
                    .send_on_init(LogSinkOnInitRequest {
                        buffer: Some(iob),
                        interest: Some(Interest {
                            min_severity: Some(min_severity.severity),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })
                    .unwrap();
                min_severity.updates = MinSeverityUpdates::None;
            }

            while let Some(Ok(request)) = requests.next().await {
                match request {
                    LogSinkRequest::WaitForInterestChange { responder } => {
                        let mut min_severity = min_severity.lock();
                        match &mut min_severity.updates {
                            MinSeverityUpdates::None => {
                                min_severity.updates =
                                    MinSeverityUpdates::Listeners(vec![responder]);
                            }
                            MinSeverityUpdates::Pending => {
                                let _ = responder.send(Ok(&Interest {
                                    min_severity: Some(min_severity.severity),
                                    ..Default::default()
                                }));
                                min_severity.updates = MinSeverityUpdates::None;
                            }
                            MinSeverityUpdates::Listeners(l) => l.push(responder),
                        }
                    }
                    _ => unreachable!(),
                }
            }
        });
    }

    /// Sets the minimum severity and notifies all listeners.
    pub fn set_min_severity(&self, severity: Severity) {
        let mut min_severity = self.min_severity.lock();
        if severity == min_severity.severity {
            return;
        }
        min_severity.severity = severity;
        match &mut min_severity.updates {
            MinSeverityUpdates::Listeners(l) => {
                for responder in l.drain(..) {
                    let _ = responder
                        .send(Ok(&Interest { min_severity: Some(severity), ..Default::default() }));
                }
                min_severity.updates = MinSeverityUpdates::None;
            }
            _ => min_severity.updates = MinSeverityUpdates::Pending,
        }
    }
}

#[cfg(feature = "ffi")]
pub mod ffi {
    // NOTE: It isn't currently possible to link to more than one rustc_staticlib; it results in
    // duplicate definition linker errors on LTO builds. To workaround this, we import
    // log_decoder_c_bindings here so that it gets pulled in as part of this static library.
    use log_decoder_c_bindings as _;

    use super::FakeLogSink;
    use fidl::endpoints::ServerEnd;
    use fidl_fuchsia_diagnostics_types::Severity;
    use fuchsia_async::TimeoutExt;
    use futures::executor::block_on;

    /// Creates a new fake log sink.
    #[unsafe(no_mangle)]
    pub extern "C" fn fake_log_sink_new() -> *mut FakeLogSink {
        Box::into_raw(Box::new(super::FakeLogSink::new_impl(true)))
    }

    /// Deletes a log sink.
    ///
    /// # Safety
    ///
    /// `fake` must be from `fake_log_sink_new()`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn fake_log_sink_delete(fake: *mut FakeLogSink) {
        drop(unsafe { Box::from_raw(fake) });
    }

    /// Serves a new connection
    ///
    /// # Safety
    ///
    /// `fake` must be from `fake_log_sink_new()` and `handle` must be valid.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn fake_log_sink_serve(fake: *mut FakeLogSink, handle: u32) {
        unsafe { &*fake }
            .serve(ServerEnd::new(unsafe { zx::NullableHandle::from_raw(handle) }.into()));
    }

    /// Reads a new record.
    ///
    ///
    /// # Safety
    ///
    /// `fake` must be from `fake_log_sink_new()` and `dest` and `capacity` must be valid.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn fake_log_sink_read_record(
        fake: *mut FakeLogSink,
        dest: *mut u8,
        capacity: usize,
    ) -> usize {
        let fake = unsafe { &mut *fake };
        let buf = block_on(fake.ring_buffer.read_message()).unwrap().1;
        assert!(buf.len() <= capacity, "{} {capacity}", buf.len());
        unsafe {
            std::slice::from_raw_parts_mut(dest, buf.len()).copy_from_slice(&buf);
        }
        buf.len()
    }

    /// Sets the minimum severity and notifies listeners.
    ///
    /// # Panics
    ///
    /// This will panic if `severity` is invalid.
    ///
    /// # Safety
    ///
    /// `fake` must be from `fake_log_sink_new()`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn fake_log_sink_set_min_severity(fake: *mut FakeLogSink, severity: u8) {
        unsafe { &*fake }.set_min_severity(Severity::from_primitive(severity).unwrap());
    }

    /// Waits for a record to be ready and returns its size, or zero if timed out.
    ///
    /// # Safety
    ///
    /// `fake` must be from `fake_log_sink_new()`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn fake_log_sink_wait_for_record(
        fake: *mut FakeLogSink,
        deadline_nanos: i64,
    ) -> usize {
        unsafe fn erase_lifetime(sink: &mut FakeLogSink) -> &'static mut FakeLogSink {
            unsafe { std::mem::transmute(sink) }
        }

        let fake = unsafe { &mut *fake };

        let tail = fake.ring_buffer.tail();
        let scope = fake.ehandle.global_scope().clone();

        {
            let fake = unsafe { erase_lifetime(fake) };
            block_on(scope.compute(async move {
                fake.ring_buffer
                    .wait(tail)
                    .on_timeout(zx::MonotonicInstant::from_nanos(deadline_nanos), || 0)
                    .await
            }));
        }

        let head = fake.ring_buffer.head();
        if head == tail {
            0
        } else {
            unsafe { fake.ring_buffer.first_message_in(tail..head) }.unwrap().1.len()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FakeLogSink;
    use diagnostics_log_encoding::encode::{
        Encoder, EncoderOpts, LogEvent, MutableBuffer, WriteEventParams,
    };
    use fidl::endpoints::create_proxy;
    use fidl_fuchsia_logger::{LogSinkEvent, LogSinkOnInitRequest, MAX_DATAGRAM_LEN_BYTES};
    use futures::StreamExt;
    use std::io::Cursor;

    #[fuchsia::test(logging = false)]
    async fn log() {
        let (proxy, server) = create_proxy();
        let mut fake_sink = FakeLogSink::new();
        fake_sink.serve(server);

        // NOTE: This can be changed to use the diagnostics client library when support for the
        // IOBuffer has been added.
        let Some(Ok(LogSinkEvent::OnInit {
            payload: LogSinkOnInitRequest { buffer: Some(iob), .. },
        })) = proxy.take_event_stream().next().await
        else {
            panic!("Expected OnInit")
        };

        let mut buf = [0u8; MAX_DATAGRAM_LEN_BYTES as _];
        let mut encoder = Encoder::new(Cursor::new(&mut buf[..]), EncoderOpts::default());
        let tags: &[&str] = &[];
        const MSG: &str = "The quick brown fox jumps over the lazy dog";
        encoder
            .write_event(WriteEventParams {
                event: LogEvent::new(&log::Record::builder().args(format_args!("{MSG}")).build()),
                tags,
                metatags: std::iter::empty(),
                pid: zx::Koid::from_raw(1),
                tid: zx::Koid::from_raw(2),
                dropped: 0,
            })
            .unwrap();
        let end = encoder.inner().cursor();
        iob.write(Default::default(), 0, &encoder.inner().get_ref()[..end]).unwrap();

        assert_eq!(fake_sink.read_message().await, MSG);
    }
}
