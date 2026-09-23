// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::compat::FcTransportStatus;
use crate::env_context::{EnvContext, FfxConfigEntry};
use crate::ext_buffer::ExtBuffer;
use crate::lib_context::LibContext;
use fdomain_client::{AsHandleRef, HandleOp, MessageBuf, Peered};
use fidl::Rights;
use std::mem::MaybeUninit;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::task::Poll;
use zx_types;

type Responder<T> = mpsc::SyncSender<T>;
type CmdResult<T> = Result<T, FcTransportStatus>;

pub(crate) struct ReadResponse {
    pub(crate) actual_bytes_count: usize,
    pub(crate) actual_handles_count: usize,
    pub(crate) result: FcTransportStatus,
}

impl From<FcTransportStatus> for ReadResponse {
    fn from(result: FcTransportStatus) -> Self {
        Self { actual_bytes_count: 0, actual_handles_count: 0, result }
    }
}

pub(crate) enum LibraryCommand {
    ShutdownLib,
    BlockForever {
        responder: Responder<()>,
    },
    GetNotificationDescriptor {
        lib: Arc<LibContext>,
        calling_thread: std::thread::ThreadId,
        responder: Responder<i32>,
    },
    CreateEnvContext {
        lib: Arc<LibContext>,
        calling_thread: std::thread::ThreadId,
        responder: Responder<CmdResult<Arc<EnvContext>>>,
        config: Vec<FfxConfigEntry>,
        isolate_dir: Option<PathBuf>,
    },
    OpenDeviceProxy {
        env: Arc<EnvContext>,
        calling_thread: std::thread::ThreadId,
        moniker: String,
        capability_name: String,
        responder: Responder<CmdResult<zx_types::zx_handle_t>>,
    },
    OpenRemoteControlProxy {
        env: Arc<EnvContext>,
        calling_thread: std::thread::ThreadId,
        responder: Responder<CmdResult<zx_types::zx_handle_t>>,
    },
    HandleGetKoid {
        lib: Arc<LibContext>,
        calling_thread: std::thread::ThreadId,
        handle: zx_types::zx_handle_t,
        responder: Responder<CmdResult<u64>>,
    },
    ChannelRead {
        lib: Arc<LibContext>,
        calling_thread: std::thread::ThreadId,
        channel: zx_types::zx_handle_t,
        out_buf: ExtBuffer<u8>,
        out_handles: ExtBuffer<MaybeUninit<zx_types::zx_handle_t>>,
        responder: Responder<ReadResponse>,
    },
    ChannelCreate {
        env: Arc<EnvContext>,
        calling_thread: std::thread::ThreadId,
        responder: Responder<CmdResult<(zx_types::zx_handle_t, zx_types::zx_handle_t)>>,
    },
    ChannelWrite {
        lib: Arc<LibContext>,
        calling_thread: std::thread::ThreadId,
        channel: zx_types::zx_handle_t,
        buf: ExtBuffer<u8>,
        handles: ExtBuffer<zx_types::zx_handle_t>,
        responder: Responder<FcTransportStatus>,
    },
    ConfigGetString {
        env_ctx: Arc<EnvContext>,
        calling_thread: std::thread::ThreadId,
        config_key: String,
        out_buf: ExtBuffer<u8>,
        responder: Responder<CmdResult<usize>>,
    },
    ChannelWriteEtc {
        lib: Arc<LibContext>,
        calling_thread: std::thread::ThreadId,
        channel: zx_types::zx_handle_t,
        buf: ExtBuffer<u8>,
        handles: ExtBuffer<zx_types::zx_handle_disposition_t>,
        responder: Responder<FcTransportStatus>,
    },
    EventCreate {
        env: Arc<EnvContext>,
        calling_thread: std::thread::ThreadId,
        responder: Responder<CmdResult<zx_types::zx_handle_t>>,
    },
    EventPairCreate {
        env: Arc<EnvContext>,
        calling_thread: std::thread::ThreadId,
        responder: Responder<CmdResult<(zx_types::zx_handle_t, zx_types::zx_handle_t)>>,
    },
    ObjectSignal {
        lib: Arc<LibContext>,
        calling_thread: std::thread::ThreadId,
        handle: zx_types::zx_handle_t,
        clear_mask: fidl::Signals,
        set_mask: fidl::Signals,
        responder: Responder<FcTransportStatus>,
    },
    ObjectSignalPeer {
        lib: Arc<LibContext>,
        calling_thread: std::thread::ThreadId,
        handle: zx_types::zx_handle_t,
        clear_mask: fidl::Signals,
        set_mask: fidl::Signals,
        responder: Responder<FcTransportStatus>,
    },
    ObjectSignalPoll {
        lib: Arc<LibContext>,
        calling_thread: std::thread::ThreadId,
        handle: zx_types::zx_handle_t,
        signals: fidl::Signals,
        responder: Responder<CmdResult<fidl::Signals>>,
    },
    SocketCreate {
        env: Arc<EnvContext>,
        calling_thread: std::thread::ThreadId,
        options: u32,
        responder: Responder<CmdResult<(zx_types::zx_handle_t, zx_types::zx_handle_t)>>,
    },
    SocketRead {
        lib: Arc<LibContext>,
        calling_thread: std::thread::ThreadId,
        socket: zx_types::zx_handle_t,
        out_buf: ExtBuffer<u8>,
        responder: Responder<ReadResponse>,
    },
    SocketWrite {
        lib: Arc<LibContext>,
        calling_thread: std::thread::ThreadId,
        socket: zx_types::zx_handle_t,
        buf: ExtBuffer<u8>,
        responder: Responder<FcTransportStatus>,
    },
    TargetWait {
        env: Arc<EnvContext>,
        calling_thread: std::thread::ThreadId,
        timeout: u64,
        responder: Responder<FcTransportStatus>,
        offline: bool,
    },
    HandleClose {
        lib: Arc<LibContext>,
        handle: zx_types::zx_handle_t,
        responder: Responder<()>,
    },
}

impl LibraryCommand {
    pub(crate) async fn run(self) {
        match self {
            Self::ShutdownLib => panic!("unsupported command. exiting thread."),
            Self::BlockForever { responder: _r } => loop {
                tokio::time::sleep(std::time::Duration::MAX).await
            },
            Self::GetNotificationDescriptor { lib, calling_thread, responder } => {
                match lib.notifier_descriptor().await {
                    Ok(r) => {
                        responder.send(r).unwrap();
                    }
                    Err(e) => {
                        lib.write_err(calling_thread, e);
                        responder.send(FcTransportStatus::INTERNAL.into_raw()).unwrap();
                    }
                }
            }
            Self::CreateEnvContext { lib, calling_thread, responder, config, isolate_dir } => {
                match EnvContext::new(Arc::downgrade(&lib), config, isolate_dir) {
                    Ok(e) => {
                        responder.send(Ok(Arc::new(e))).unwrap();
                    }
                    Err(e) => {
                        lib.write_err(calling_thread, e);
                        responder.send(Err(FcTransportStatus::INTERNAL)).unwrap();
                    }
                }
            }
            Self::OpenRemoteControlProxy { env, calling_thread, responder } => {
                match env.connect_remote_control_proxy().await {
                    Ok(h) => {
                        responder.send(Ok(h)).unwrap();
                    }
                    Err(e) => {
                        env.write_err(calling_thread, e);
                        responder.send(Err(FcTransportStatus::INTERNAL)).unwrap();
                    }
                }
            }
            Self::HandleGetKoid { lib, calling_thread, handle, responder } => {
                let mut state = lib.fdomain_state().await;
                let handle = match state.handle(handle) {
                    Ok(h) => h,
                    Err(e) => {
                        lib.write_err(calling_thread, e);
                        responder.send(Err(FcTransportStatus::INTERNAL)).unwrap();
                        return;
                    }
                };
                match handle.as_handle_ref().get_koid().await {
                    Ok(koid) => responder.send(Ok(koid)).unwrap(),
                    Err(e) => {
                        lib.write_fdomain_err(calling_thread, &e);
                        responder.send(Err(e.into())).unwrap();
                    }
                }
            }
            Self::OpenDeviceProxy { env, calling_thread, moniker, capability_name, responder } => {
                match env.connect_device_proxy(moniker, capability_name).await {
                    Ok(r) => {
                        responder.send(Ok(r)).unwrap();
                    }
                    Err(e) => {
                        env.write_err(calling_thread, e);
                        responder.send(Err(FcTransportStatus::INTERNAL)).unwrap();
                    }
                }
            }
            Self::ChannelRead {
                lib,
                calling_thread,
                channel,
                mut out_buf,
                mut out_handles,
                responder,
            } => {
                // There's room here to optimize this. We've already got an out buffer to read
                // directly into here. For the sake of avoiding complexity this is just copying
                // things for now.
                let mut message_buf = MessageBuf::new();
                let poll_res =
                    match lib.fdomain_state().await.channel_read(channel, &mut message_buf).await {
                        Ok(p) => match p {
                            Poll::Ready(res) => res,
                            Poll::Pending => {
                                responder.send(FcTransportStatus::SHOULD_WAIT.into()).unwrap();
                                return;
                            }
                        },
                        Err(e) => {
                            lib.write_err(calling_thread, e);
                            responder.send(FcTransportStatus::INTERNAL.into()).unwrap();
                            return;
                        }
                    };
                match poll_res {
                    Ok(()) => {
                        let actual_bytes_count = message_buf.bytes.len();
                        let actual_handles_count = message_buf.handles.len();
                        // Just for whomever is reading here: this is the behavior of
                        // ZX_ERR_CHANNEL_READ_MAY_DISCARD, which is to discard the message rather
                        // than saving it for a subsequent read attempt. For the time being this is
                        // much easier to implement, and the only bindings existing have the maximum
                        // FIDL message size in use and cannot hit this message.
                        //
                        // An alternative would just be to have either FDomain channels or for the
                        // library context to save the message and then for each subsequent read we
                        // see if there's a stored message to be sent out instead of discarding.
                        if out_buf.len() < message_buf.bytes.len()
                            || out_handles.len() < message_buf.handles.len()
                        {
                            responder
                                .send(ReadResponse {
                                    actual_bytes_count,
                                    actual_handles_count,
                                    result: FcTransportStatus::BUFFER_TOO_SMALL,
                                })
                                .unwrap();
                            return;
                        }
                        out_buf[..actual_bytes_count].copy_from_slice(message_buf.bytes.as_slice());
                        let mut fdomain_state = lib.fdomain_state().await;
                        for (i, handle_info) in message_buf.handles.into_iter().enumerate() {
                            let hdl_num = fdomain_state.register(handle_info.handle.into());
                            out_handles[i] = MaybeUninit::new(hdl_num);
                        }
                        responder
                            .send(ReadResponse {
                                actual_bytes_count,
                                actual_handles_count,
                                result: FcTransportStatus::OK,
                            })
                            .unwrap();
                    }
                    Err(e) => {
                        lib.write_fdomain_err(calling_thread, &e);
                        responder.send(FcTransportStatus::from(e).into()).unwrap();
                    }
                }
            }
            Self::ChannelCreate { env, calling_thread, responder } => {
                let fdomain_client = match env.fdomain_client().await {
                    Ok(c) => c,
                    Err(e) => {
                        env.write_err(calling_thread, e);
                        responder.send(Err(FcTransportStatus::INTERNAL)).unwrap();
                        return;
                    }
                };
                let (left, right) = fdomain_client.create_channel();
                let lib = env.lib_ctx();
                let mut state = lib.fdomain_state().await;
                let left = state.register(left.into());
                let right = state.register(right.into());
                responder.send(Ok((left, right))).unwrap();
            }
            Self::ChannelWrite { lib, calling_thread, channel, buf, handles, responder } => {
                let mut fdomain_handles = Vec::new();
                let mut state = lib.fdomain_state().await;
                for hdl in handles.iter() {
                    let fd_hdl = match state.take_handle(*hdl) {
                        Ok(h) => h,
                        Err(e) => {
                            lib.write_err(calling_thread, e);
                            responder.send(FcTransportStatus::INTERNAL).unwrap();
                            return;
                        }
                    };
                    fdomain_handles.push(fd_hdl);
                }
                let handle = match state.handle(channel) {
                    Ok(h) => h,
                    Err(e) => {
                        lib.write_err(calling_thread, e);
                        responder.send(FcTransportStatus::INTERNAL).unwrap();
                        return;
                    }
                };
                let channel = handle.as_unowned::<fdomain_client::Channel>();
                let status = match channel.fdomain_write(&buf, fdomain_handles).await {
                    Ok(()) => FcTransportStatus::OK,
                    Err(e) => {
                        lib.write_fdomain_err(calling_thread, &e);
                        e.into()
                    }
                };
                responder.send(status).unwrap();
            }
            Self::ChannelWriteEtc { lib, calling_thread, channel, buf, mut handles, responder } => {
                let mut handle_ops =
                    Vec::with_capacity(zx_types::ZX_CHANNEL_MAX_MSG_HANDLES as usize);
                // The verification pass is just to eliminate some headaches around lifetime checks
                // regarding the allocation of channels from raw handle numbers.
                for i in 0..handles.len() {
                    let disp = handles[i];
                    // For the time being we're still not supporting non-move ops. Though with
                    // FDomain it should now be doable.
                    if disp.operation != zx_types::ZX_HANDLE_OP_MOVE {
                        log::warn!(
                            "Unsupported handle operation type received: {}",
                            disp.operation
                        );
                        responder.send(FcTransportStatus::NOT_SUPPORTED).unwrap();
                        return;
                    }
                    if Rights::from_bits(disp.rights).is_none() {
                        responder.send(FcTransportStatus::INVALID_ARGS).unwrap();
                        return;
                    }
                }
                let mut fdomain = lib.fdomain_state().await;
                for i in 0..handles.len() {
                    let disp = std::mem::replace(
                        &mut handles[i],
                        zx_types::zx_handle_disposition_t {
                            operation: 0,
                            handle: 0,
                            type_: 0,
                            result: 0,
                            rights: 0,
                        },
                    );
                    let fdomain_hdl = match fdomain.take_handle(disp.handle) {
                        Ok(h) => h,
                        Err(e) => {
                            lib.write_err(calling_thread, e);
                            responder.send(FcTransportStatus::INTERNAL).unwrap();
                            return;
                        }
                    };
                    // SAFETY: There should have been a `Rights::from_bits` call above verifying
                    // this is not `None`.
                    handle_ops.push(HandleOp::Move(
                        fdomain_hdl,
                        Rights::from_bits(disp.rights)
                            .expect("handle rights should have been verified"),
                    ));
                }
                let handle = match fdomain.handle(channel) {
                    Ok(g) => g,
                    Err(e) => {
                        lib.write_err(calling_thread, e);
                        responder.send(FcTransportStatus::INTERNAL).unwrap();
                        return;
                    }
                };
                let channel = handle.as_unowned::<fdomain_client::Channel>();
                let status = match channel.fdomain_write_etc(&buf, handle_ops).await {
                    Ok(_) => FcTransportStatus::OK,
                    Err(e) => {
                        lib.write_fdomain_err(calling_thread, &e);
                        e.into()
                    }
                };
                responder.send(status).unwrap();
            }
            Self::ConfigGetString {
                env_ctx,
                calling_thread,
                responder,
                config_key,
                mut out_buf,
            } => {
                let result: String = match env_ctx.context.get(&config_key) {
                    Ok(r) => r,
                    Err(e) => {
                        env_ctx.write_err(calling_thread, e);
                        responder.send(Err(FcTransportStatus::NOT_FOUND)).unwrap();
                        return;
                    }
                };
                let result_bytes = result.as_bytes();
                if out_buf.len() < result_bytes.len() {
                    responder.send(Err(FcTransportStatus::BUFFER_TOO_SMALL)).unwrap();
                    return;
                }
                out_buf[..result_bytes.len()].copy_from_slice(result_bytes);
                responder.send(Ok(result_bytes.len())).unwrap();
            }
            Self::EventCreate { env, calling_thread, responder } => {
                let fdomain_client = match env.fdomain_client().await {
                    Ok(c) => c,
                    Err(e) => {
                        env.write_err(calling_thread, e);
                        responder.send(Err(FcTransportStatus::INTERNAL)).unwrap();
                        return;
                    }
                };
                let hdl = fdomain_client.create_event();
                let lib = env.lib_ctx();
                let mut fdomain_state = lib.fdomain_state().await;
                let hdl = fdomain_state.register(hdl.into());
                responder.send(Ok(hdl)).unwrap();
            }
            Self::EventPairCreate { env, calling_thread, responder } => {
                let fdomain_client = match env.fdomain_client().await {
                    Ok(c) => c,
                    Err(e) => {
                        env.write_err(calling_thread, e);
                        responder.send(Err(FcTransportStatus::INTERNAL)).unwrap();
                        return;
                    }
                };
                let (left, right) = fdomain_client.create_event_pair();
                let lib = env.lib_ctx();
                let mut state = lib.fdomain_state().await;
                let left = state.register(left.into());
                let right = state.register(right.into());
                responder.send(Ok((left, right))).unwrap();
            }
            Self::ObjectSignal { lib, calling_thread, handle, clear_mask, set_mask, responder } => {
                let mut fdomain = lib.fdomain_state().await;
                let handle = match fdomain.handle(handle) {
                    Ok(g) => g,
                    Err(e) => {
                        lib.write_err(calling_thread, e);
                        responder.send(FcTransportStatus::INTERNAL).unwrap();
                        return;
                    }
                };
                let status = match handle.signal_handle(clear_mask, set_mask).await {
                    Ok(_) => FcTransportStatus::OK,
                    Err(e) => {
                        lib.write_fdomain_err(calling_thread, &e);
                        e.into()
                    }
                };
                responder.send(status).unwrap();
            }
            Self::ObjectSignalPeer {
                lib,
                calling_thread,
                handle,
                clear_mask,
                set_mask,
                responder,
            } => {
                let mut fdomain = lib.fdomain_state().await;
                let handle = match fdomain.handle(handle) {
                    Ok(g) => g,
                    Err(e) => {
                        lib.write_err(calling_thread, e);
                        responder.send(FcTransportStatus::INTERNAL).unwrap();
                        return;
                    }
                };
                // Any handle that has a peer can be converted into an EventPair.
                let ep = handle.as_unowned::<fdomain_client::EventPair>();
                let status = match ep.signal_peer(clear_mask, set_mask).await {
                    Ok(_) => FcTransportStatus::OK,
                    Err(e) => {
                        lib.write_fdomain_err(calling_thread, &e);
                        e.into()
                    }
                };
                responder.send(status).unwrap();
            }
            Self::ObjectSignalPoll { lib, calling_thread, handle, signals, responder } => {
                let poll_res = match lib.fdomain_state().await.poll_signal(handle, signals).await {
                    Ok(p) => p,
                    Err(e) => {
                        lib.write_err(calling_thread, e);
                        responder.send(Err(FcTransportStatus::INTERNAL)).unwrap();
                        return;
                    }
                };
                let res = match poll_res {
                    Poll::Ready(res) => match res {
                        Ok(sig) => Ok(sig),
                        Err(e) => {
                            lib.write_fdomain_err(calling_thread, &e);
                            responder.send(Err(e.into())).unwrap();
                            return;
                        }
                    },
                    Poll::Pending => Err(FcTransportStatus::SHOULD_WAIT),
                };
                responder.send(res).unwrap();
            }
            Self::SocketCreate { env, calling_thread, options, responder } => {
                let fdomain_client = match env.fdomain_client().await {
                    Ok(c) => c,
                    Err(e) => {
                        env.write_err(calling_thread, e);
                        responder.send(Err(FcTransportStatus::INTERNAL)).unwrap();
                        return;
                    }
                };
                let (left, right) = match options {
                    zx_types::ZX_SOCKET_STREAM => fdomain_client.create_stream_socket(),
                    zx_types::ZX_SOCKET_DATAGRAM => fdomain_client.create_datagram_socket(),
                    o => {
                        log::warn!("Received unknown socket options: {o}");
                        responder.send(Err(FcTransportStatus::INVALID_ARGS)).unwrap();
                        return;
                    }
                };
                let lib = env.lib_ctx();
                let mut fdomain_state = lib.fdomain_state().await;
                let left = fdomain_state.register(left.into());
                let right = fdomain_state.register(right.into());
                responder.send(Ok((left, right))).unwrap();
            }
            Self::SocketRead { lib, calling_thread, socket, mut out_buf, responder } => {
                let poll_res =
                    match lib.fdomain_state().await.socket_read(socket, &mut out_buf).await {
                        Ok(p) => match p {
                            Poll::Ready(res) => res,
                            Poll::Pending => {
                                responder.send(FcTransportStatus::SHOULD_WAIT.into()).unwrap();
                                return;
                            }
                        },
                        Err(e) => {
                            lib.write_err(calling_thread, e);
                            responder.send(FcTransportStatus::INTERNAL.into()).unwrap();
                            return;
                        }
                    };
                match poll_res {
                    Err(e) => {
                        lib.write_fdomain_err(calling_thread, &e);
                        responder.send(FcTransportStatus::from(e).into()).unwrap()
                    }
                    Ok(size) => responder
                        .send(ReadResponse {
                            actual_handles_count: 0,
                            actual_bytes_count: size,
                            result: FcTransportStatus::OK,
                        })
                        .unwrap(),
                }
            }
            Self::SocketWrite { lib, calling_thread, socket, buf, responder } => {
                let mut fdomain_state = lib.fdomain_state().await;
                let handle = match fdomain_state.handle(socket) {
                    Ok(g) => g,
                    Err(e) => {
                        lib.write_err(calling_thread, e);
                        responder.send(FcTransportStatus::INTERNAL).unwrap();
                        return;
                    }
                };
                let socket = handle.as_unowned::<fdomain_client::Socket>();
                let status = match socket.fdomain_write_all(&buf).await {
                    Ok(()) => FcTransportStatus::OK,
                    Err(e) => {
                        lib.write_fdomain_err(calling_thread, &e);
                        e.into()
                    }
                };
                responder.send(status).unwrap();
            }
            Self::TargetWait { env, calling_thread, timeout, responder, offline } => {
                match env.target_wait(timeout, offline).await {
                    Ok(()) => {
                        responder.send(FcTransportStatus::OK).unwrap();
                    }
                    Err(e) => {
                        env.write_err(calling_thread, e);
                        responder.send(FcTransportStatus::INTERNAL).unwrap();
                    }
                }
            }
            Self::HandleClose { lib, handle, responder } => {
                let res = lib.fdomain_state().await.take_handle(handle);
                if let Ok(h) = res {
                    if let Err(e) = h.close().await {
                        // It's unclear so far what to do in the event of this scenario. If we lose
                        // connection to the device, we're going to lose all other handle
                        // connections, so maybe there's no point bothering to raise an exception
                        // for attempting to close the handle.
                        //
                        // That being said, since this is FDomain, a failure to close a handle for
                        // other reasons (since we have to do this remotely on the target device),
                        // may present issues in other ways, but this is likely predicated on
                        // trying to synchronize around channel closures in strange ways.
                        //
                        // For the time being, we'll log a warning, but we may want to revisit
                        // whether we should surface an error to the caller in the event that this
                        // kind of thing surfaces bugs in existing code.
                        log::warn!("Failed closing handle {handle}: {e}");
                    }
                } else {
                    log::trace!("Attempted to close {handle} but no handle found");
                }
                responder.send(()).unwrap();
            }
        }
    }
}
