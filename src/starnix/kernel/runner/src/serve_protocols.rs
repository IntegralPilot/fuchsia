// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::Container;
use anyhow::{Context as _, Error};
use fidl::endpoints::{RequestStream, ServerEnd};
use fidl_fuchsia_component_runner as frunner;
use fidl_fuchsia_element as felement;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_memory_attribution as fattribution;
use fidl_fuchsia_posix as fposix;
use fidl_fuchsia_starnix_binder as fbinder;
use fidl_fuchsia_starnix_container as fstarcontainer;
use fuchsia_async::{
    DurationExt, {self as fasync},
};
use fuchsia_rcu::RcuReadScope;
use futures::channel::oneshot;
use futures::{
    AsyncReadExt, AsyncWriteExt, Future, FutureExt, StreamExt, TryFutureExt, TryStreamExt, pin_mut,
    select,
};
use starnix_core::execution::{create_init_child_process, execute_task_with_prerun_result};
use starnix_core::fs::devpts::create_main_and_replica;
use starnix_core::fs::fuchsia::create_fuchsia_pipe;
use starnix_core::task::dynamic_thread_spawner::SpawnRequestBuilder;
use starnix_core::task::{CurrentTask, ExitStatus, Kernel, ProcessEntryRef};
use starnix_core::vfs::buffers::{VecInputBuffer, VecOutputBuffer};
use starnix_core::vfs::file_server::serve_file_at;
use starnix_core::vfs::socket::VsockSocket;
use starnix_core::vfs::{FdFlags, FileHandle};
use starnix_logging::{log_error, log_warn};
use starnix_modules_framebuffer::Framebuffer;
use starnix_task_command::TaskCommand;
use starnix_uapi::auth::Credentials;
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::signals::UncheckedSignal;
use starnix_uapi::{errno, error, uapi};
use std::ffi::CString;

use super::start_component;

pub fn expose_root(
    system_task: &CurrentTask,
    server_end: ServerEnd<fio::DirectoryMarker>,
) -> Result<(), Error> {
    let root_file = system_task.open_file("/".into(), OpenFlags::RDONLY)?;
    serve_file_at(server_end.into_channel().into(), system_task, &root_file, Credentials::root())?;
    Ok(())
}

pub async fn serve_component_runner(
    request_stream: frunner::ComponentRunnerRequestStream,
    system_task: &CurrentTask,
) -> Result<(), Error> {
    request_stream
        .try_for_each_concurrent(None, |event| async {
            match event {
                frunner::ComponentRunnerRequest::Start { start_info, controller, .. } => {
                    if let Err(e) = start_component(start_info, controller, system_task).await {
                        log_error!("failed to start component: {:?}", e);
                    }
                }
                frunner::ComponentRunnerRequest::_UnknownMethod { ordinal, .. } => {
                    log_warn!("Unknown ComponentRunner request: {ordinal}");
                }
            }
            Ok(())
        })
        .await
        .map_err(Error::from)
}

fn to_winsize(window_size: Option<fstarcontainer::ConsoleWindowSize>) -> uapi::winsize {
    window_size
        .map(|window_size| uapi::winsize {
            ws_row: window_size.rows,
            ws_col: window_size.cols,
            ws_xpixel: window_size.x_pixels,
            ws_ypixel: window_size.y_pixels,
        })
        .unwrap_or(uapi::winsize::default())
}

async fn spawn_console(
    kernel: &Kernel,
    payload: fstarcontainer::ControllerSpawnConsoleRequest,
) -> Result<Result<u8, fstarcontainer::SpawnConsoleError>, Error> {
    if let (Some(console_in), Some(console_out), Some(binary_path)) =
        (payload.console_in, payload.console_out, payload.binary_path)
    {
        let binary_path = CString::new(binary_path)?;
        let argv = payload
            .argv
            .unwrap_or(vec![])
            .into_iter()
            .map(CString::new)
            .collect::<Result<Vec<_>, _>>()?;
        let environ = payload
            .environ
            .unwrap_or(vec![])
            .into_iter()
            .map(CString::new)
            .collect::<Result<Vec<_>, _>>()?;
        let window_size = to_winsize(payload.window_size);
        let current_task = create_init_child_process(
            &kernel.weak_self.upgrade().expect("Kernel must still be alive"),
            TaskCommand::new(binary_path.as_bytes()),
            Credentials::with_ids(0, 0),
            None,
        )?;
        let (sender, receiver) = oneshot::channel();
        let pty = execute_task_with_prerun_result(
            current_task,
            move |current_task| {
                let executable = current_task.open_file_for_exec(
                    starnix_core::vfs::FdNumber::AT_FDCWD,
                    binary_path.as_bytes().into(),
                    OpenFlags::empty(),
                )?;
                current_task.exec(executable, binary_path, argv, environ)?;
                let (pty, pts) = create_main_and_replica(&current_task, window_size)?;
                let fd_flags = FdFlags::empty();
                assert_eq!(0, current_task.add_file(pts.clone(), fd_flags)?.raw());
                assert_eq!(1, current_task.add_file(pts.clone(), fd_flags)?.raw());
                assert_eq!(2, current_task.add_file(pts, fd_flags)?.raw());
                Ok(pty)
            },
            move |result| {
                let _ = match result {
                    Ok(ExitStatus::Exit(exit_code)) => sender.send(Ok(exit_code)),
                    _ => sender.send(Err(fstarcontainer::SpawnConsoleError::Canceled)),
                };
            },
            None,
        )?;
        let _ = forward_to_pty(kernel, console_in, console_out, pty).map_err(|e| {
            log_error!("failed to forward to terminal {:?}", e);
        });

        Ok(receiver.await?)
    } else {
        Ok(Err(fstarcontainer::SpawnConsoleError::InvalidArgs))
    }
}

pub async fn serve_container_controller(
    request_stream: fstarcontainer::ControllerRequestStream,
    system_task: &CurrentTask,
) -> Result<(), Error> {
    request_stream
        .map_err(Error::from)
        .try_for_each_concurrent(None, |event| async {
            match event {
                fstarcontainer::ControllerRequest::VsockConnect {
                    payload:
                        fstarcontainer::ControllerVsockConnectRequest { port, bridge_socket, .. },
                    ..
                } => {
                    let Some(port) = port else {
                        log_warn!("vsock connection missing port");
                        return Ok(());
                    };
                    let Some(bridge_socket) = bridge_socket else {
                        log_warn!("vsock connection missing bridge_socket");
                        return Ok(());
                    };
                    connect_to_vsock(port, bridge_socket, system_task).await.unwrap_or_else(|e| {
                        log_error!("failed to connect to vsock {:?}", e);
                    });
                }
                fstarcontainer::ControllerRequest::SpawnConsole { payload, responder } => {
                    responder.send(spawn_console(system_task.kernel(), payload).await?)?;
                }
                fstarcontainer::ControllerRequest::GetVmoReferences { payload, responder } => {
                    if let Some(koid) = payload.koid {
                        let mut results = vec![];
                        for thread_group in
                            system_task.kernel().pids.get_thread_groups(&RcuReadScope::new())
                        {
                            if let Ok(leader) = thread_group.leader.get_task() {
                                if let Ok(files) = leader.files() {
                                    let fds = files.get_all_fds();
                                    for fd in fds {
                                        if let Ok(file) = files.get(fd) {
                                            if let Ok(memory) = file.get_memory(
                                                system_task,
                                                None,
                                                starnix_core::mm::ProtectionFlags::READ,
                                            ) {
                                                let memory_koid = memory
                                                    .info()
                                                    .expect("Failed to get memory info")
                                                    .koid;
                                                if memory_koid.raw_koid() == koid {
                                                    let process_name = thread_group
                                                        .process
                                                        .get_name()
                                                        .unwrap_or_default();
                                                    results.push(fstarcontainer::VmoReference {
                                                        process_name: Some(
                                                            process_name.to_string(),
                                                        ),
                                                        pid: Some(leader.get_pid() as u64),
                                                        fd: Some(fd.raw()),
                                                        koid: Some(koid),
                                                        ..Default::default()
                                                    });
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        let _ =
                            responder.send(&fstarcontainer::ControllerGetVmoReferencesResponse {
                                references: Some(results),
                                ..Default::default()
                            });
                    }
                }
                fstarcontainer::ControllerRequest::GetJobHandle { responder } => {
                    let _result = responder.send(fstarcontainer::ControllerGetJobHandleResponse {
                        job: Some(
                            fuchsia_runtime::job_default()
                                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                                .expect("Failed to dup handle"),
                        ),
                        ..Default::default()
                    });
                }
                fstarcontainer::ControllerRequest::SendSignal {
                    payload:
                        fstarcontainer::ControllerSendSignalRequest {
                            pid: Some(pid),
                            signal: Some(signal),
                            ..
                        },
                    responder,
                } => {
                    let pids = &system_task.kernel().pids;
                    if let Some(ProcessEntryRef::Process(target_thread_group)) =
                        pids.get(pid).ok().and_then(|p| p.get_process())
                    {
                        #[allow(
                            clippy::undocumented_unsafe_blocks,
                            reason = "Force documented unsafe blocks in Starnix"
                        )]
                        match unsafe {
                            target_thread_group.send_signal_unchecked_debug(
                                system_task,
                                UncheckedSignal::new(signal),
                            )
                        } {
                            Ok(_) => {
                                let _result = responder.send(Ok(()));
                            }
                            Err(_) => {
                                let _result =
                                    responder.send(Err(fstarcontainer::SignalError::InvalidSignal));
                            }
                        };
                    } else {
                        let _result =
                            responder.send(Err(fstarcontainer::SignalError::InvalidTarget));
                    };
                }
                // The request did not contain both a signal and a target pid.
                fstarcontainer::ControllerRequest::SendSignal { responder, .. } => {
                    log_error!("malformed SendSignal request");
                    let _result = responder.send(Err(fstarcontainer::SignalError::InvalidTarget));
                }
                fstarcontainer::ControllerRequest::SetSyscallLogFilter { payload, responder } => {
                    if let Some(process_name) = payload.process_name {
                        system_task.kernel().add_syscall_log_filter(&process_name);
                        let _ = responder.send(Ok(()));
                    } else {
                        let _ = responder.send(Err(
                            fstarcontainer::SetSyscallLogFilterError::MissingProcessName,
                        ));
                    }
                }
                fstarcontainer::ControllerRequest::ClearSyscallLogFilters { responder } => {
                    system_task.kernel().clear_syscall_log_filters();
                    let _ = responder.send();
                }
                fstarcontainer::ControllerRequest::_UnknownMethod { .. } => (),
            }
            Ok(())
        })
        .await
}

async fn connect_to_vsock(
    port: u32,
    bridge_socket: fidl::Socket,
    system_task: &CurrentTask,
) -> Result<(), Error> {
    let socket = loop {
        if let Ok(socket) = system_task.kernel().default_abstract_vsock_namespace.lookup(&port) {
            break socket;
        };
        fasync::Timer::new(fasync::MonotonicDuration::from_millis(100).after_now()).await;
    };

    let pipe =
        create_fuchsia_pipe(system_task, bridge_socket, OpenFlags::RDWR | OpenFlags::NONBLOCK)?;
    socket.downcast_socket::<VsockSocket>().unwrap().remote_connection(
        &socket,
        system_task,
        pipe,
    )?;

    Ok(())
}

fn forward_to_pty(
    kernel: &Kernel,
    console_in: fidl::Socket,
    console_out: fidl::Socket,
    pty: FileHandle,
) -> Result<(), Error> {
    // Matches fuchsia.io.Transfer capacity, somewhat arbitrarily.
    const BUFFER_CAPACITY: usize = 8192;

    let mut rx = fuchsia_async::Socket::from_socket(console_in);
    let mut tx = fuchsia_async::Socket::from_socket(console_out);
    let pty_sink = pty.clone();
    let closure = async move |current_task: &CurrentTask| {
        let _result: Result<(), Error> = (async || {
            let mut buffer = vec![0u8; BUFFER_CAPACITY];
            loop {
                let bytes = rx.read(&mut buffer[..]).await?;
                if bytes == 0 {
                    return Ok(());
                }
                pty_sink.write(current_task, &mut VecInputBuffer::new(&buffer[..bytes]))?;
            }
        })()
        .await;
    };
    let req = SpawnRequestBuilder::new()
        .with_debug_name("forward-to-pty-in")
        .with_async_closure(closure)
        .build();
    kernel.kthreads.spawner().spawn_from_request(req);

    let pty_source = pty;
    let closure = move |current_task: &CurrentTask| {
        let _result: Result<(), Error> =
            fasync::LocalExecutor::default().run_singlethreaded(async {
                let mut buffer = VecOutputBuffer::new(BUFFER_CAPACITY);
                loop {
                    buffer.reset();
                    let bytes = pty_source.read(current_task, &mut buffer)?;
                    if bytes == 0 {
                        return Ok(());
                    }
                    tx.write_all(buffer.data()).await?;
                }
            });
    };
    let req = SpawnRequestBuilder::new()
        .with_debug_name("forward-to-pty-out")
        .with_sync_closure(closure)
        .build();
    kernel.kthreads.spawner().spawn_from_request(req);

    Ok(())
}

pub async fn serve_container_info(
    request_stream: fstarcontainer::InfoRequestStream,
) -> Result<(), Error> {
    request_stream
        .map_err(Error::from)
        .try_for_each_concurrent(None, |event| async {
            match event {
                fstarcontainer::InfoRequest::GetJobKoid { responder } => {
                    let koid = fuchsia_runtime::job_default()
                        .koid()
                        .unwrap_or(zx::Koid::from_raw(zx::sys::ZX_KOID_INVALID));
                    responder.send(&fstarcontainer::InfoGetJobKoidResponse {
                        koid: Some(koid.raw_koid()),
                        ..Default::default()
                    })?;
                }
                fstarcontainer::InfoRequest::_UnknownMethod { ordinal, .. } => {
                    starnix_logging::log_warn!("Unknown InfoRequest method: {}", ordinal);
                }
            }
            Ok(())
        })
        .await
}

pub async fn serve_graphical_presenter(
    mut request_stream: felement::GraphicalPresenterRequestStream,
    kernel: &Kernel,
) -> Result<(), Error> {
    while let Some(request) = request_stream.next().await {
        match request.context("reading graphical presenter request")? {
            felement::GraphicalPresenterRequest::PresentView {
                view_spec,
                annotation_controller: _,
                view_controller_request: _,
                responder,
            } => match view_spec.viewport_creation_token {
                Some(token) => {
                    let fb = Framebuffer::get(kernel).context("getting framebuffer from kernel")?;
                    fb.present_view(token);
                    let _ = responder.send(Ok(()));
                }
                None => {
                    let _ = responder.send(Err(felement::PresentViewError::InvalidArgs));
                }
            },
        }
    }
    Ok(())
}

/// Serves the memory attribution provider for the Kernel ELF component.
pub fn serve_memory_attribution_provider_elfkernel(
    mut request_stream: fattribution::ProviderRequestStream,
    container: &Container,
) -> impl Future<Output = Result<(), Error>> {
    let observer = container.new_memory_attribution_observer(request_stream.control_handle());
    async move {
        while let Some(event) = request_stream.try_next().await? {
            match event {
                fattribution::ProviderRequest::Get { responder } => {
                    observer.next(responder);
                }
                fattribution::ProviderRequest::_UnknownMethod {
                    ordinal, control_handle, ..
                } => {
                    log_error!("Invalid request to AttributionProvider: {ordinal}");
                    control_handle.shutdown_with_epitaph(zx::Status::INVALID_ARGS);
                }
            }
        }
        Ok(())
    }
}

/// Serves the memory attribution provider for the Container component.
pub fn serve_memory_attribution_provider_container(
    mut request_stream: fattribution::ProviderRequestStream,
    kernel: &Kernel,
) -> impl Future<Output = ()> + use<> {
    let observer = kernel.new_memory_attribution_observer(request_stream.control_handle());
    async move {
        while let Some(event) = request_stream
            .try_next()
            .await
            .inspect_err(|err| {
                log_warn!("Error while serving container memory attribution: {:?}", err)
            })
            .ok()
            .flatten()
        {
            match event {
                fattribution::ProviderRequest::Get { responder } => {
                    observer.next(responder);
                }
                fattribution::ProviderRequest::_UnknownMethod {
                    ordinal, control_handle, ..
                } => {
                    log_error!("Invalid request to AttributionProvider: {ordinal}");
                    control_handle.shutdown_with_epitaph(zx::Status::INVALID_ARGS);
                }
            }
        }
    }
}

async fn select_first<O>(f1: impl Future<Output = O>, f2: impl Future<Output = O>) -> O {
    let f1 = f1.fuse();
    let f2 = f2.fuse();
    pin_mut!(f1, f2);
    select! {
        f1 = f1 => f1,
        f2 = f2 => f2,
    }
}

/// Serve the LutexController protocol.
pub async fn serve_lutex_controller(
    request_stream: fbinder::LutexControllerRequestStream,
    current_task: &CurrentTask,
) -> Result<(), Error> {
    let kernel = current_task.kernel();
    request_stream
        .map_err(Error::from)
        .try_for_each_concurrent(None, |event| async move {
            match event {
                fbinder::LutexControllerRequest::WaitBitset { payload, responder } => {
                    let deadline_and_receiver = (|| {
                        let vmo = payload.vmo.ok_or_else(|| errno!(EINVAL))?;
                        let offset = payload.offset.ok_or_else(|| errno!(EINVAL))?;
                        let value = payload.value.ok_or_else(|| errno!(EINVAL))?;
                        let mask = payload.mask.unwrap_or(u32::MAX);
                        let deadline = payload.deadline.map(zx::MonotonicInstant::from_nanos);
                        kernel
                            .shared_futexes
                            .external_wait(vmo.into(), offset, value, mask)
                            .map(|(event, receiver)| (deadline, event, receiver))
                    })();
                    let result = match deadline_and_receiver {
                        Ok((deadline, event, receiver)) => {
                            // We construct a specific `wait_fut` to explicitly bind the lifecycle
                            // of the `event` to the duration of this wait operation. If the FIDL
                            // client disconnects (or the future is otherwise dropped/cancelled),
                            // the `event` is immediately dropped. This zeros the strong reference
                            // count on the `InterruptibleEvent`, signaling to the `FutexTable`
                            // that this external waiter is now stale and should be garbage
                            // collected on its next cleanup pass.
                            let wait_fut = async move {
                                let _event = event;
                                let receiver = receiver.map_err(|_| errno!(EINTR));
                                if let Some(deadline) = deadline {
                                    let timer =
                                        fasync::Timer::new(deadline).map(|_| error!(ETIMEDOUT));
                                    select_first(timer, receiver).await
                                } else {
                                    receiver.await
                                }
                            };
                            wait_fut.await
                        }
                        Err(e) => Err(e),
                    };
                    let result = result.map_err(|e: Errno| {
                        fposix::Errno::from_primitive(e.code.error_code() as i32)
                            .unwrap_or(fposix::Errno::Einval)
                    });
                    responder
                        .send(result)
                        .context("Unable to send LutexControllerRequest::WaitBitset response")?;
                }
                fbinder::LutexControllerRequest::WakeBitset { payload, responder } => {
                    let result = (|| {
                        let vmo = payload.vmo.ok_or_else(|| errno!(EINVAL))?;
                        let offset = payload.offset.ok_or_else(|| errno!(EINVAL))?;
                        let count = payload.count.ok_or_else(|| errno!(EINVAL))?;
                        let mask = payload.mask.unwrap_or(u32::MAX);
                        kernel.shared_futexes.external_wake(
                            vmo.into(),
                            offset,
                            count as usize,
                            mask,
                        )
                    })();
                    let result = result
                        .map(|count| fbinder::WakeResponse {
                            count: Some(count as u64),
                            ..fbinder::WakeResponse::default()
                        })
                        .map_err(|e: Errno| {
                            fposix::Errno::from_primitive(e.code.error_code() as i32)
                                .unwrap_or(fposix::Errno::Einval)
                        });
                    responder
                        .send(result)
                        .context("Unable to send LutexControllerRequest::WakeBitset response")?;
                }
                fbinder::LutexControllerRequest::CmpRequeue { payload, responder } => {
                    let result = (|| {
                        let first_vmo = payload.first_vmo.ok_or_else(|| errno!(EINVAL))?;
                        let first_offset = payload.first_offset.ok_or_else(|| errno!(EINVAL))?;
                        let second_vmo = payload.second_vmo;
                        let second_offset = payload.second_offset.ok_or_else(|| errno!(EINVAL))?;
                        let wake_count = payload.wake_count.ok_or_else(|| errno!(EINVAL))?;
                        let requeue_count = payload.requeue_count.ok_or_else(|| errno!(EINVAL))?;
                        let cmp_val = payload.cmp_val;
                        kernel.shared_futexes.external_requeue(
                            first_vmo.into(),
                            first_offset,
                            second_vmo.map(Into::into),
                            second_offset,
                            wake_count as usize,
                            requeue_count as usize,
                            cmp_val,
                        )
                    })();
                    let result = result
                        .map(|count| fbinder::CmpRequeueResponse {
                            count: Some(count as u64),
                            ..fbinder::CmpRequeueResponse::default()
                        })
                        .map_err(|e: Errno| {
                            fposix::Errno::from_primitive(e.code.error_code() as i32)
                                .unwrap_or(fposix::Errno::Einval)
                        });
                    responder
                        .send(result)
                        .context("Unable to send LutexControllerRequest::Requeue response")?;
                }
                fbinder::LutexControllerRequest::_UnknownMethod { ordinal, .. } => {
                    log_warn!("Unknown LutexController ordinal: {}", ordinal);
                }
            }
            Ok(())
        })
        .await
        .context("failed fbinder::LutexController request")
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fidl_fuchsia_posix as fposix;
    use starnix_core::testing::*;
    use zx;

    #[fuchsia::test]
    async fn lutex_controller_test() {
        spawn_kernel_and_run(async |current_task| {
            let (sender, receiver) = oneshot::channel::<()>();
            current_task.kernel.kthreads.spawn_future(
                {
                    let kernel = current_task.kernel.clone();
                    move || async move {
                        let (lutex_controller, stream) = fidl::endpoints::create_proxy_and_stream::<
                            fbinder::LutexControllerMarker,
                        >();

                        // Spawn the server
                        let server_fut =
                            serve_lutex_controller(stream, kernel.kthreads.system_task());

                        let client_fut = async move {
                            const VMO_SIZE: usize = 4 * 1024;
                            let vmo = zx::Vmo::create(VMO_SIZE as u64).expect("Vmo::create");
                            // Wait on an incorrect value.
                            let wait = lutex_controller
                                .wait_bitset(fbinder::WaitBitsetRequest {
                                    vmo: Some(
                                        vmo.duplicate_handle(zx::Rights::SAME_RIGHTS)
                                            .expect("duplicate vmo"),
                                    ),
                                    offset: Some(0),
                                    value: Some(1),
                                    ..Default::default()
                                })
                                .await
                                .expect("got_answer");
                            assert_matches!(wait, Err(fposix::Errno::Eagain));

                            // Wait with a timeout
                            let wait = lutex_controller
                                .wait_bitset(fbinder::WaitBitsetRequest {
                                    vmo: Some(
                                        vmo.duplicate_handle(zx::Rights::SAME_RIGHTS)
                                            .expect("duplicate vmo"),
                                    ),
                                    offset: Some(0),
                                    value: Some(0),
                                    deadline: Some(0),
                                    ..Default::default()
                                })
                                .await
                                .expect("got_answer");
                            assert_matches!(wait, Err(fposix::Errno::Etimedout));

                            let mut wait = Box::pin(
                                lutex_controller.wait_bitset(fbinder::WaitBitsetRequest {
                                    vmo: Some(
                                        vmo.duplicate_handle(zx::Rights::SAME_RIGHTS)
                                            .expect("duplicate vmo"),
                                    ),
                                    offset: Some(0),
                                    value: Some(0),
                                    deadline: None,
                                    ..Default::default()
                                }),
                            );
                            // The wait is correct, the future should stay pending until a wake.
                            assert!(futures::poll!(&mut wait).is_pending());

                            let waken_up: fbinder::WakeResponse = lutex_controller
                                .wake_bitset(fbinder::WakeBitsetRequest {
                                    vmo: Some(
                                        vmo.duplicate_handle(zx::Rights::SAME_RIGHTS)
                                            .expect("duplicate vmo"),
                                    ),
                                    offset: Some(0),
                                    count: Some(1),
                                    mask: None,
                                    ..Default::default()
                                })
                                .await
                                .expect("wake_answer")
                                .expect("wake_response");
                            assert_eq!(waken_up.count, Some(1));

                            // The wait should now return.
                            assert!(wait.await.expect("await_answer").is_ok());

                            // test cmp_requeue
                            {
                                let vmo2 = zx::Vmo::create(VMO_SIZE as u64).expect("Vmo::create");
                                let mut wait1 = Box::pin(
                                    lutex_controller.wait_bitset(fbinder::WaitBitsetRequest {
                                        vmo: Some(
                                            vmo.duplicate_handle(zx::Rights::SAME_RIGHTS)
                                                .expect("duplicate vmo"),
                                        ),
                                        offset: Some(4),
                                        value: Some(0),
                                        deadline: None,
                                        ..Default::default()
                                    }),
                                );
                                let mut wait2 = Box::pin(
                                    lutex_controller.wait_bitset(fbinder::WaitBitsetRequest {
                                        vmo: Some(
                                            vmo.duplicate_handle(zx::Rights::SAME_RIGHTS)
                                                .expect("duplicate vmo"),
                                        ),
                                        offset: Some(4),
                                        value: Some(0),
                                        deadline: None,
                                        ..Default::default()
                                    }),
                                );
                                let cmp_requeue_response = lutex_controller
                                    .cmp_requeue(fbinder::CmpRequeueRequest {
                                        first_vmo: Some(
                                            vmo.duplicate_handle(zx::Rights::SAME_RIGHTS)
                                                .expect("duplicate vmo"),
                                        ),
                                        first_offset: Some(4),
                                        second_vmo: Some(
                                            vmo2.duplicate_handle(zx::Rights::SAME_RIGHTS)
                                                .expect("duplicate vmo"),
                                        ),
                                        second_offset: Some(8),
                                        wake_count: Some(1),
                                        requeue_count: Some(1),
                                        cmp_val: Some(0xBAD),
                                        ..Default::default()
                                    })
                                    .await
                                    .expect("got_answer")
                                    .unwrap_err();
                                // 0xBAD is not 0x0, so EAGAIN expected
                                assert_matches!(cmp_requeue_response, fposix::Errno::Eagain);
                                let cmp_requeue_response2: fbinder::CmpRequeueResponse =
                                    lutex_controller
                                        .cmp_requeue(fbinder::CmpRequeueRequest {
                                            first_vmo: Some(
                                                vmo.duplicate_handle(zx::Rights::SAME_RIGHTS)
                                                    .expect("duplicate vmo"),
                                            ),
                                            first_offset: Some(4),
                                            second_vmo: Some(
                                                vmo2.duplicate_handle(zx::Rights::SAME_RIGHTS)
                                                    .expect("duplicate vmo"),
                                            ),
                                            second_offset: Some(8),
                                            wake_count: Some(1),
                                            requeue_count: Some(1),
                                            cmp_val: Some(0x0),
                                            ..Default::default()
                                        })
                                        .await
                                        .expect("got_answer")
                                        .expect("got_response");
                                // 1 woken, 1 re-queued
                                assert_matches!(cmp_requeue_response2.count, Some(2));
                                // give wait1 and wait2 a chance to complete; we don't rely on this
                                // being long enough however
                                fasync::Timer::new(
                                    fasync::MonotonicDuration::from_millis(100).after_now(),
                                )
                                .await;
                                let mut still_pending_count: u32 = 0;
                                // Up to one of wait1 and wait2 can be completed by this point, so at
                                // least one must still be pending.
                                let wait1_poll = futures::poll!(&mut wait1);
                                if wait1_poll.is_pending() {
                                    still_pending_count += 1;
                                }
                                let wait2_poll = futures::poll!(&mut wait2);
                                if wait2_poll.is_pending() {
                                    still_pending_count += 1;
                                }
                                assert!(still_pending_count >= 1);
                                // at this point exactly one of wait1 or wait2 will remain pending until
                                // we wake it via vmo2 - we don't know which of wait1 or wait2 we're
                                // waking here (which is fine)
                                let vmo2_wake_response: fbinder::WakeResponse = lutex_controller
                                    .wake_bitset(fbinder::WakeBitsetRequest {
                                        vmo: Some(
                                            vmo2.duplicate_handle(zx::Rights::SAME_RIGHTS)
                                                .expect("duplicate vmo"),
                                        ),
                                        offset: Some(8),
                                        count: Some(1),
                                        mask: None,
                                        ..Default::default()
                                    })
                                    .await
                                    .expect("wake_answer")
                                    .expect("wake_response");
                                assert_eq!(vmo2_wake_response.count, Some(1));
                                // Now we know that both wait1 and wait2 are unblocked, so we can
                                // wait on both of them.
                                if wait1_poll.is_pending() {
                                    assert!(wait1.await.expect("await_answer").is_ok());
                                }
                                if wait2_poll.is_pending() {
                                    assert!(wait2.await.expect("await_answer").is_ok());
                                }
                            }
                        };

                        let (server_res, _) = futures::join!(server_fut, client_fut);
                        server_res.expect("server failed");
                        let _ = sender.send(());
                    }
                },
                "lutex_controller_test",
            );
            receiver.await.expect("test failed");
        })
        .await;
    }

    #[fuchsia::test]
    async fn container_info_test() {
        let (info_proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fstarcontainer::InfoMarker>();

        let server_fut = serve_container_info(stream);
        let client_fut = async move {
            let response = info_proxy.get_job_koid().await.expect("get_job_koid failed");
            let expected_koid = fuchsia_runtime::job_default()
                .koid()
                .unwrap_or(zx::Koid::from_raw(zx::sys::ZX_KOID_INVALID));
            assert_eq!(response.koid, Some(expected_koid.raw_koid()));
            assert_ne!(response.koid, Some(zx::sys::ZX_KOID_INVALID));
            drop(info_proxy);
        };

        let (server_res, ()) = futures::join!(server_fut, client_fut);
        server_res.expect("server failed");
    }
}
