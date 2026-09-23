// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::commands::LibraryCommand;
use crate::fdomain::FDomainState;
use anyhow::Result;
use async_lock::{Mutex as AsyncMutex, MutexGuard};
use fdomain_client::Error as FDomainInternalError;
use fuchsia_async::{LocalExecutor, Task};
use signal_hook::consts::signal;
use std::os::fd::{IntoRawFd, RawFd};
use std::sync::{Arc, Mutex};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;
use zx_types;

pub type Notifier = Arc<AsyncMutex<Option<LibNotifier>>>;

#[derive(Debug)]
pub(crate) enum ErrorPayload {
    String(String),
    Fidl(Vec<u8>),
}

pub struct LibContext {
    pub(crate) errors: Mutex<std::collections::HashMap<std::thread::ThreadId, ErrorPayload>>,
    notifier: Notifier,
    cmd_sender: async_channel::Sender<LibraryCommand>,
    thread_ctx: Mutex<Option<std::thread::JoinHandle<()>>>,
    signal_handler_thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    fdomain_state: AsyncMutex<FDomainState>,
    signal_handle: signal_hook::iterator::Handle,
}

impl LibContext {
    pub(crate) fn new() -> Self {
        let notifier = Notifier::default();
        let (cmd_sender, receiver) = async_channel::unbounded::<LibraryCommand>();
        let (signal_sender, signal_receiver) = async_channel::bounded(1);
        let mut sig_watcher =
            signal_hook::iterator::Signals::new(&[signal::SIGINT, signal::SIGTERM]).unwrap();
        let signal_handle = sig_watcher.handle();
        let signal_handler_thread = std::thread::spawn(move || {
            // Signal behavior for both is the same: say we've been interrupted and then let the
            // above handle that.
            for s in sig_watcher.forever() {
                let debug_str = match s {
                    signal::SIGINT => "SIGINT",
                    signal::SIGTERM => "SIGTERM",
                    _ => unreachable!(),
                };
                log::info!("Received signal '{debug_str}'. Sending interrupt message to thread.");
                match signal_sender.try_send(s) {
                    Ok(()) => {
                        log::info!("signal interrupt sent successfully");
                    }
                    Err(e) => match e {
                        async_channel::TrySendError::Full(_) => {
                            log::info!("signal interrupt queue full.");
                        }
                        async_channel::TrySendError::Closed(_) => {
                            log::info!("signal interupt queue closed.");
                            break;
                        }
                    },
                }
            }
        });
        Self {
            cmd_sender,
            errors: Mutex::new(std::collections::HashMap::new()),
            notifier: notifier.clone(),
            thread_ctx: Mutex::new(Some(new_command_thread(
                receiver,
                signal_receiver,
                notifier.clone(),
            ))),
            fdomain_state: AsyncMutex::new(FDomainState::new(notifier)),
            signal_handler_thread: Mutex::new(Some(signal_handler_thread)),
            signal_handle,
        }
    }

    fn fidl_to_payload(&self, fidl_err: &impl fidl::Persistable) -> Option<ErrorPayload> {
        Some(ErrorPayload::Fidl(fidl::persist(fidl_err).expect("encoding fdomain fidl error")))
    }

    pub(crate) fn write_fdomain_err(
        &self,
        thread_id: std::thread::ThreadId,
        err: &FDomainInternalError,
    ) {
        use FDomainInternalError::*;
        let payload = match err {
            SocketWrite(s) => self.fidl_to_payload(s),
            ChannelWrite(c) => self.fidl_to_payload(c),
            FDomain(f) => self.fidl_to_payload(f),
            ProtocolObjectTypeIncompatible
            | ProtocolRightsIncompatible
            | ConnectionMismatch
            | StreamingAborted
            | ProtocolSignalsIncompatible
            | ProtocolStreamEventIncompatible => None,
            t @ Transport(_) => Some(ErrorPayload::String(self.err_to_string(t))),
            p @ Protocol(_) => Some(ErrorPayload::String(self.err_to_string(p))),
        };
        if let Some(p) = payload {
            let mut guard = self.errors.lock().unwrap();
            guard.insert(thread_id, p);
        }
    }

    fn err_to_string<T: std::fmt::Display>(&self, err: T) -> String {
        // LINT.IfChange(no_fdomain_client)
        let error = format!("FFX Library Error: {err}");
        // LINT.ThenChange(//tools/testing/tefmocheck/string_in_log_check.go:no_fdomain_client)
        error
    }

    pub(crate) fn write_err<T: std::fmt::Display>(&self, thread_id: std::thread::ThreadId, err: T) {
        let mut guard = self.errors.lock().unwrap();
        guard.insert(thread_id, ErrorPayload::String(self.err_to_string(err)));
    }

    pub(crate) fn run(&self, cmd: LibraryCommand) {
        // Should not fail as this is an unbounded channel. In the future, when
        // updating to more recent versions of the async_channel library, this
        // can be handled using send_blocking instead.
        self.cmd_sender.try_send(cmd).expect("Sending to command channel");
    }

    pub(crate) async fn notifier_descriptor(&self) -> Result<RawFd> {
        let mut notifier = self.notifier.lock().await;
        if !notifier.is_some() {
            *notifier = Some(LibNotifier::new()?);
        }
        Ok(notifier.as_ref().unwrap().receiver())
    }

    pub(crate) fn shutdown_cmd_thread(&self) {
        log::info!("Shutting down lib context thread.");
        self.run(LibraryCommand::ShutdownLib);
        let thread =
            self.thread_ctx.lock().unwrap().take().expect("thread context must have been set");
        assert_ne!(
            std::thread::current().id(),
            thread.thread().id(),
            "thread is being dropped from inside itself"
        );
        thread.join().expect("joining thread");
        log::info!("Thread successfully joined.");

        // Signal the signal handler thread to exit.
        self.signal_handle.close();

        log::info!("Joining signal thread");
        let signal_thread = self
            .signal_handler_thread
            .lock()
            .unwrap()
            .take()
            .expect("signal handler thread must have been set");
        assert_ne!(
            std::thread::current().id(),
            signal_thread.thread().id(),
            "thread is being dropped from inside itself"
        );
        signal_thread.join().expect("joining signal thread");
        log::info!("Signal thread joined successfully.");
    }

    pub(crate) async fn fdomain_state<'a>(&'a self) -> MutexGuard<'a, FDomainState> {
        self.fdomain_state.lock().await
    }
}

fn new_command_thread(
    receiver: async_channel::Receiver<LibraryCommand>,
    sigint_receiver: async_channel::Receiver<i32>,
    notifier: Notifier,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(|| {
        log::info!("Starting lib_context main thread");
        let mut executor = LocalExecutor::default();
        executor.run_singlethreaded(async move {
            let pool = std::cell::RefCell::new(fuchsia_async::Scope::new());
            while futures_lite::FutureExt::or(
                async {
                    let Ok(cmd) = receiver.recv().await else {
                        return false;
                    };
                    if let LibraryCommand::ShutdownLib = cmd {
                        return false;
                    }
                    pool.borrow().spawn_local(cmd.run());
                    true
                },
                async {
                    let Ok(s) = sigint_receiver.recv().await else {
                        return false;
                    };
                    log::info!("command thread received signal");
                    // SIGINT: interrupt all pending commands. Continue thread.
                    // SIGTERM: exit thread. Abandon everything.
                    match s {
                        signal::SIGINT => {
                            let old_pool = pool.replace(fuchsia_async::Scope::new());
                            old_pool.cancel().await;
                            true
                        }
                        signal::SIGTERM => false,
                        _ => panic!("received unhandled signal: {s}"),
                    }
                },
            )
            .await
            {}
            // Dropping the notifier will cause spawned tasks to be dropped.
            *notifier.lock().await = None;
            pool.into_inner().cancel().await;
        });
        log::info!("Exiting lib_context main thread");
    })
}

pub(crate) struct LibNotifier {
    _pipe_reader_task: Task<()>,
    handle_notification_sender: async_channel::Sender<zx_types::zx_handle_t>,
    stream_fd: RawFd,
}

impl LibNotifier {
    // This function isn't actually async, but it should be called inside an
    // executor to ensure spawned tasks are scheduled correctly.
    fn new() -> Result<Self> {
        let (stream_rx, mut stream_tx) = UnixStream::pair()?;
        let (tx, rx) = async_channel::unbounded::<zx_types::zx_handle_t>();
        let pipe_reader_task = fuchsia_async::Task::local(async move {
            while let Ok(raw_handle) = rx.recv().await {
                match stream_tx.write_u32_le(raw_handle).await {
                    Ok(_) => {}
                    Err(e) => {
                        log::info!("Exiting pipe reader task. Error: {e:?}");
                        break;
                    }
                }
            }
        });
        Ok(Self {
            handle_notification_sender: tx,
            _pipe_reader_task: pipe_reader_task,
            stream_fd: stream_rx.into_std()?.into_raw_fd(),
        })
    }

    fn receiver(&self) -> RawFd {
        self.stream_fd
    }

    pub fn sender(&self) -> async_channel::Sender<zx_types::zx_handle_t> {
        self.handle_notification_sender.clone()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn testing_lib_context() -> LibContext {
        LibContext::new()
    }

    // Tests for a bug in which `Transport(None)` would simply print `None`.
    #[test]
    fn test_transport_error_prints_readable_message() {
        let ctx = testing_lib_context();
        let err = FDomainInternalError::Transport(None);
        let thread_id = std::thread::current().id();
        ctx.write_fdomain_err(thread_id, &err);
        let errors = ctx.errors.lock().unwrap();
        let payload = errors.get(&thread_id).unwrap();
        if let ErrorPayload::String(s) = payload {
            assert!(s.contains(&format!("{err}")), "GOT: '{s}'; WANT: '{err}'");
        } else {
            panic!("Expected string error payload");
        }
    }
}
