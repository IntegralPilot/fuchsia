// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Given this is exposing a C ABI, it is clear that most of the functionality is going to be
// accessing raw pointers unsafely. This disables the individual unsafe function checks for a
// `# Safety` section.
#![allow(clippy::missing_safety_doc)]
// Shows more granularity over unsafe operations inside unsafe functions.
#![deny(unsafe_op_in_unsafe_fn)]

use crate::commands::{LibraryCommand, ReadResponse};
use crate::compat::FcTransportStatus;
use crate::env_context::{EnvContext, FfxConfigEntry};
use crate::ext_buffer::ExtBuffer;
use crate::lib_context::LibContext;
use std::ffi::CStr;
use std::mem::MaybeUninit;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use zx_types;

mod commands;
mod compat;
mod env_context;
mod ext_buffer;
mod fdomain;
mod lib_context;
mod logging;
mod waker;

// LINT.IfChange

#[unsafe(no_mangle)]
pub unsafe extern "C" fn create_ffx_lib_context(ctx: *mut *const LibContext) {
    let ctx_out = LibContext::new();
    let ptr = Arc::into_raw(Arc::new(ctx_out));

    unsafe { *ctx = ptr };
}

#[derive(Debug)]
#[repr(C)]
pub struct FfxExternalConfigEntry {
    pub key: *const i8,
    pub value: *const i8,
}

unsafe fn get_arc<T>(ptr: *const T) -> Arc<T> {
    unsafe { Arc::increment_strong_count(ptr) };
    unsafe { Arc::from_raw(ptr) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn create_ffx_env_context(
    env_ctx: *mut *const EnvContext,
    lib_ctx: *const LibContext,
    external_config: *const FfxExternalConfigEntry,
    config_len: u64,
    isolate_dir: *const i8,
) -> FcTransportStatus {
    let lib = unsafe { get_arc(lib_ctx) };
    let isolate_dir = unsafe { isolate_dir.as_ref() }.map(|i| {
        PathBuf::from(unsafe {
            CStr::from_ptr(i).to_str().expect("value isolate dir string").to_owned()
        })
    });
    let (responder, rx) = mpsc::sync_channel(1);
    let mut config = Vec::new();
    if external_config != std::ptr::null_mut() {
        for i in 0..TryInto::<isize>::try_into(config_len).unwrap() {
            let config_entry: &FfxExternalConfigEntry = unsafe { &*external_config.offset(i) };
            let key = unsafe {
                CStr::from_ptr(config_entry.key).to_str().expect("valid config string").to_owned()
            };
            let value = unsafe {
                CStr::from_ptr(config_entry.value).to_str().expect("valid config string").to_owned()
            };
            config.push(FfxConfigEntry { key, value });
        }
    }
    let calling_thread = std::thread::current().id();
    lib.run(LibraryCommand::CreateEnvContext {
        lib: lib.clone(),
        calling_thread,
        responder,
        config,
        isolate_dir,
    });
    match rx.recv() {
        Ok(r) => match r {
            Ok(env) => {
                unsafe { *env_ctx = Arc::into_raw(env) };
                FcTransportStatus::OK
            }
            Err(e) => e,
        },
        Err(_) => FcTransportStatus::INTERRUPTED,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffx_connect_device_proxy(
    ctx: *mut EnvContext,
    moniker: *const i8,
    capability_name: *const i8,
    handle: *mut zx_types::zx_handle_t,
) -> FcTransportStatus {
    let moniker = unsafe { CStr::from_ptr(moniker) }.to_str().expect("valid moniker").to_owned();
    let capability_name = unsafe { CStr::from_ptr(capability_name) }
        .to_str()
        .expect("valid capability name")
        .to_owned();
    let ctx = unsafe { get_arc(ctx) };
    let (responder, rx) = mpsc::sync_channel(1);
    let calling_thread = std::thread::current().id();
    ctx.lib_ctx().run(LibraryCommand::OpenDeviceProxy {
        env: ctx.clone(),
        calling_thread,
        moniker,
        capability_name,
        responder,
    });
    match rx.recv() {
        Ok(r) => match r {
            Ok(h) => {
                unsafe { *handle = h };
                FcTransportStatus::OK
            }
            Err(e) => e,
        },
        Err(_) => FcTransportStatus::INTERRUPTED,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffx_target_wait(
    ctx: *mut EnvContext,
    timeout: u64,
    offline: bool,
) -> FcTransportStatus {
    let ctx = unsafe { get_arc(ctx) };
    let (responder, rx) = mpsc::sync_channel(1);
    let calling_thread = std::thread::current().id();
    ctx.lib_ctx().run(LibraryCommand::TargetWait {
        env: ctx.clone(),
        calling_thread,
        timeout,
        responder,
        offline,
    });
    rx.recv().unwrap_or(FcTransportStatus::INTERRUPTED).into()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffx_connect_remote_control_proxy(
    ctx: *mut EnvContext,
    handle: *mut zx_types::zx_handle_t,
) -> FcTransportStatus {
    let ctx = unsafe { get_arc(ctx) };
    let (responder, rx) = mpsc::sync_channel(1);
    let calling_thread = std::thread::current().id();
    ctx.lib_ctx().run(LibraryCommand::OpenRemoteControlProxy {
        env: ctx.clone(),
        calling_thread,
        responder,
    });
    match rx.recv() {
        Ok(r) => match r {
            Ok(h) => {
                unsafe { *handle = h };
                FcTransportStatus::OK
            }
            Err(e) => e,
        },
        Err(_) => FcTransportStatus::INTERRUPTED,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn destroy_ffx_lib_context(ctx: *const LibContext) {
    if ctx != std::ptr::null() {
        let ctx = unsafe { Arc::from_raw(ctx) };
        ctx.shutdown_cmd_thread();
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn destroy_ffx_env_context(ctx: *const EnvContext) {
    if ctx != std::ptr::null() {
        drop(unsafe { Arc::from_raw(ctx) });
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffx_close_handle(
    ctx: *const LibContext,
    handle: zx_types::zx_handle_t,
) -> FcTransportStatus {
    let ctx = unsafe { get_arc(ctx) };
    let (responder, rx) = mpsc::sync_channel(1);
    ctx.run(LibraryCommand::HandleClose { lib: ctx.clone(), handle, responder });
    rx.recv().map(|_| FcTransportStatus::OK).unwrap_or(FcTransportStatus::INTERRUPTED)
}

fn safe_write<T>(dest: *mut T, value: T) {
    let dest = unsafe { dest.as_mut() };
    dest.map(|d| *d = value);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffx_channel_write(
    ctx: *const LibContext,
    channel: zx_types::zx_handle_t,
    out_buf: *mut u8,
    out_len: u64,
    hdls: *mut zx_types::zx_handle_t,
    hdls_len: u64,
) -> FcTransportStatus {
    let ctx = unsafe { get_arc(ctx) };
    let (responder, rx) = mpsc::sync_channel(1);
    let calling_thread = std::thread::current().id();
    ctx.run(LibraryCommand::ChannelWrite {
        lib: ctx.clone(),
        calling_thread,
        channel,
        buf: unsafe { ExtBuffer::new(out_buf, out_len as usize) },
        handles: unsafe { ExtBuffer::new(hdls, hdls_len as usize) },
        responder,
    });
    rx.recv().unwrap_or(FcTransportStatus::INTERRUPTED).into()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffx_channel_write_etc(
    ctx: *const LibContext,
    handle: zx_types::zx_handle_t,
    out_buf: *mut u8,
    out_len: u64,
    hdls: *mut zx_types::zx_handle_disposition_t,
    hdls_len: u64,
) -> FcTransportStatus {
    let ctx = unsafe { get_arc(ctx) };
    let (responder, rx) = mpsc::sync_channel(1);
    let calling_thread = std::thread::current().id();
    ctx.run(LibraryCommand::ChannelWriteEtc {
        lib: ctx.clone(),
        calling_thread,
        channel: handle,
        buf: unsafe { ExtBuffer::new(out_buf, out_len as usize) },
        // Construction of HandleDisposition structs has to happen in the main thread, as it
        // contains a lifetime bound.
        handles: unsafe { ExtBuffer::new(hdls, hdls_len as usize) },
        responder,
    });
    rx.recv().unwrap_or(FcTransportStatus::INTERRUPTED).into()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffx_channel_read(
    ctx: *const LibContext,
    handle: zx_types::zx_handle_t,
    out_buf: *mut u8,
    out_len: u64,
    hdls: *mut zx_types::zx_handle_t,
    hdls_len: u64,
    actual_bytes_count: *mut u64,
    actual_hdls_count: *mut u64,
) -> FcTransportStatus {
    let ctx = unsafe { get_arc(ctx) };
    let (responder, rx) = mpsc::sync_channel(1);
    let calling_thread = std::thread::current().id();
    ctx.run(LibraryCommand::ChannelRead {
        lib: ctx.clone(),
        calling_thread,
        channel: handle,
        out_buf: unsafe { ExtBuffer::new(out_buf, out_len as usize) },
        out_handles: unsafe {
            ExtBuffer::new(hdls as *mut MaybeUninit<zx_types::zx_handle_t>, hdls_len as usize)
        },
        responder,
    });
    let ReadResponse {
        actual_bytes_count: bytes_count_recv,
        actual_handles_count: handles_count_recv,
        result,
    } = match rx.recv() {
        Ok(r) => r,
        Err(_) => return FcTransportStatus::INTERRUPTED,
    };
    safe_write(actual_bytes_count, bytes_count_recv as u64);
    safe_write(actual_hdls_count, handles_count_recv as u64);
    result.into()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffx_socket_create(
    ctx: *const EnvContext,
    options: u32,
    out0: *mut zx_types::zx_handle_t,
    out1: *mut zx_types::zx_handle_t,
) -> FcTransportStatus {
    let ctx = unsafe { get_arc(ctx) };
    let (tx, rx) = mpsc::sync_channel(1);
    let calling_thread = std::thread::current().id();
    ctx.lib_ctx().run(LibraryCommand::SocketCreate {
        env: ctx.clone(),
        calling_thread,
        options,
        responder: tx,
    });
    match rx.recv() {
        Ok(res) => match res {
            Ok((ch0, ch1)) => {
                unsafe { *out0 = ch0 };
                unsafe { *out1 = ch1 };
                FcTransportStatus::OK
            }
            Err(e) => e,
        },
        Err(_) => FcTransportStatus::INTERRUPTED,
    }
    .into()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffx_socket_write(
    ctx: *const LibContext,
    handle: zx_types::zx_handle_t,
    buf: *mut u8,
    buf_len: u64,
) -> FcTransportStatus {
    let ctx = unsafe { get_arc(ctx) };
    let (responder, rx) = mpsc::sync_channel(1);
    let calling_thread = std::thread::current().id();
    ctx.run(LibraryCommand::SocketWrite {
        lib: ctx.clone(),
        calling_thread,
        socket: handle,
        buf: unsafe { ExtBuffer::new(buf, buf_len as usize) },
        responder,
    });
    rx.recv().unwrap_or(FcTransportStatus::INTERRUPTED).into()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffx_socket_read(
    ctx: *const LibContext,
    handle: zx_types::zx_handle_t,
    out_buf: *mut u8,
    out_len: u64,
    bytes_read: *mut u64,
) -> FcTransportStatus {
    let ctx = unsafe { get_arc(ctx) };
    let (responder, rx) = mpsc::sync_channel(1);
    let calling_thread = std::thread::current().id();
    ctx.run(LibraryCommand::SocketRead {
        lib: ctx.clone(),
        calling_thread,
        socket: handle,
        out_buf: unsafe { ExtBuffer::new(out_buf, out_len as usize) },
        responder,
    });
    let ReadResponse { actual_bytes_count: bytes_count_recv, result, .. } = match rx.recv() {
        Ok(r) => r,
        Err(_) => return FcTransportStatus::INTERRUPTED,
    };
    safe_write(bytes_read, bytes_count_recv as u64);
    result.into()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffx_connect_handle_notifier(ctx: *const LibContext) -> i32 {
    let ctx = unsafe { get_arc(ctx) };
    let (tx, rx) = mpsc::sync_channel(1);
    let calling_thread = std::thread::current().id();
    ctx.run(LibraryCommand::GetNotificationDescriptor {
        lib: ctx.clone(),
        calling_thread,
        responder: tx,
    });
    rx.recv().unwrap_or_else(|_| FcTransportStatus::INTERRUPTED.into_raw())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffx_event_create(
    ctx: *const EnvContext,
    _options: u32,
    out: *mut zx_types::zx_handle_t,
) -> FcTransportStatus {
    let ctx = unsafe { get_arc(ctx) };
    let (tx, rx) = mpsc::sync_channel(1);
    let calling_thread = std::thread::current().id();
    ctx.lib_ctx().run(LibraryCommand::EventCreate {
        env: ctx.clone(),
        calling_thread,
        responder: tx,
    });
    match rx.recv() {
        Ok(r) => match r {
            Ok(hdl) => {
                unsafe { *out = hdl };
                FcTransportStatus::OK
            }
            Err(e) => e,
        },
        Err(_) => FcTransportStatus::INTERRUPTED,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffx_eventpair_create(
    ctx: *const EnvContext,
    _options: u32,
    out0: *mut zx_types::zx_handle_t,
    out1: *mut zx_types::zx_handle_t,
) -> FcTransportStatus {
    let ctx = unsafe { get_arc(ctx) };
    let (tx, rx) = mpsc::sync_channel(1);
    let calling_thread = std::thread::current().id();
    ctx.lib_ctx().run(LibraryCommand::EventPairCreate {
        env: ctx.clone(),
        calling_thread,
        responder: tx,
    });
    match rx.recv() {
        Ok(r) => match r {
            Ok((hdl0, hdl1)) => {
                unsafe { *out0 = hdl0 };
                unsafe { *out1 = hdl1 };
                FcTransportStatus::OK
            }
            Err(e) => e,
        },
        Err(_) => FcTransportStatus::INTERRUPTED,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffx_object_signal(
    ctx: *const LibContext,
    handle: zx_types::zx_handle_t,
    clear_mask: u32,
    set_mask: u32,
) -> FcTransportStatus {
    let ctx = unsafe { get_arc(ctx) };
    let (tx, rx) = mpsc::sync_channel(1);
    let clear_mask = fidl::Signals::from_bits_retain(clear_mask);
    let set_mask = fidl::Signals::from_bits_retain(set_mask);
    let calling_thread = std::thread::current().id();
    ctx.run(LibraryCommand::ObjectSignal {
        lib: ctx.clone(),
        calling_thread,
        handle,
        clear_mask,
        set_mask,
        responder: tx,
    });
    rx.recv().unwrap_or(FcTransportStatus::INTERRUPTED).into()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffx_object_signal_peer(
    ctx: *const LibContext,
    handle: zx_types::zx_handle_t,
    clear_mask: u32,
    set_mask: u32,
) -> FcTransportStatus {
    let ctx = unsafe { get_arc(ctx) };
    let (tx, rx) = mpsc::sync_channel(1);
    let clear_mask = fidl::Signals::from_bits_retain(clear_mask);
    let set_mask = fidl::Signals::from_bits_retain(set_mask);
    let calling_thread = std::thread::current().id();
    ctx.run(LibraryCommand::ObjectSignalPeer {
        lib: ctx.clone(),
        calling_thread,
        handle,
        clear_mask,
        set_mask,
        responder: tx,
    });
    rx.recv().unwrap_or(FcTransportStatus::INTERRUPTED).into()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffx_object_signal_poll(
    ctx: *const LibContext,
    handle: zx_types::zx_handle_t,
    signals: u32,
    signals_out: *mut u32,
) -> FcTransportStatus {
    let ctx = unsafe { get_arc(ctx) };
    let (tx, rx) = mpsc::sync_channel(1);
    let signals = fidl::Signals::from_bits_retain(signals);
    let calling_thread = std::thread::current().id();
    ctx.run(LibraryCommand::ObjectSignalPoll {
        lib: ctx.clone(),
        calling_thread,
        handle,
        signals,
        responder: tx,
    });
    match rx.recv() {
        Ok(r) => match r {
            Ok(sig) => {
                safe_write(signals_out, sig.bits());
                FcTransportStatus::OK
            }
            Err(status) => status,
        },
        Err(_) => FcTransportStatus::INTERRUPTED,
    }
    .into()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffx_channel_create(
    ctx: *const EnvContext,
    _options: u32,
    out0: *mut zx_types::zx_handle_t,
    out1: *mut zx_types::zx_handle_t,
) -> FcTransportStatus {
    let ctx = unsafe { get_arc(ctx) };
    let (tx, rx) = mpsc::sync_channel(1);
    let calling_thread = std::thread::current().id();
    ctx.lib_ctx().run(LibraryCommand::ChannelCreate {
        env: ctx.clone(),
        calling_thread,
        responder: tx,
    });
    match rx.recv() {
        Ok(r) => match r {
            Ok((hdl0, hdl1)) => {
                unsafe { *out0 = hdl0 };
                unsafe { *out1 = hdl1 };
                FcTransportStatus::OK
            }
            Err(e) => e,
        },
        Err(_) => FcTransportStatus::INTERRUPTED,
    }
    .into()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffx_handle_get_koid(
    ctx: *const LibContext,
    handle: zx_types::zx_handle_t,
    out: *mut zx_types::zx_koid_t,
) -> FcTransportStatus {
    let ctx = unsafe { get_arc(ctx) };
    let (tx, rx) = mpsc::sync_channel(1);
    let calling_thread = std::thread::current().id();
    ctx.run(LibraryCommand::HandleGetKoid {
        lib: ctx.clone(),
        calling_thread,
        handle,
        responder: tx,
    });
    match rx.recv() {
        Ok(r) => match r {
            Ok(k) => {
                unsafe { *out = k };
                FcTransportStatus::OK
            }
            Err(e) => e,
        },
        Err(_) => FcTransportStatus::INTERRUPTED,
    }
    .into()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _block_forever(ctx: *const LibContext) -> FcTransportStatus {
    let ctx = unsafe { get_arc(ctx) };
    let (tx, rx) = mpsc::sync_channel(1);
    ctx.run(LibraryCommand::BlockForever { responder: tx });
    match rx.recv() {
        Ok(_) => panic!("This branch should never be triggered"),
        Err(_) => FcTransportStatus::INTERRUPTED,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffx_config_get_string(
    ctx: *const EnvContext,
    config_key: *const u8,
    config_key_len: u64,
    out_buf: *mut u8,
    out_buf_len: *mut u64,
) -> FcTransportStatus {
    if ctx == std::ptr::null()
        || config_key == std::ptr::null()
        || out_buf == std::ptr::null_mut()
        || out_buf_len == std::ptr::null_mut()
    {
        return FcTransportStatus::INVALID_ARGS;
    }
    let ctx = unsafe { get_arc(ctx) };
    let (tx, rx) = mpsc::sync_channel(1);
    let calling_thread = std::thread::current().id();
    let config_key = unsafe { std::slice::from_raw_parts(config_key, config_key_len as usize) };
    let config_key_str = match std::str::from_utf8(config_key) {
        Ok(s) => s.to_owned(),
        Err(e) => {
            ctx.write_err(calling_thread, e);
            return FcTransportStatus::INTERNAL;
        }
    };
    let out_buf_size = unsafe { *out_buf_len as usize };
    ctx.lib_ctx().run(LibraryCommand::ConfigGetString {
        responder: tx,
        env_ctx: ctx.clone(),
        calling_thread,
        config_key: config_key_str,
        out_buf: unsafe { ExtBuffer::new(out_buf, out_buf_size) },
    });
    match rx.recv() {
        Ok(r) => match r {
            Ok(size) => {
                safe_write(out_buf_len, size as u64);
                FcTransportStatus::OK
            }
            Err(e) => e,
        },
        Err(_) => FcTransportStatus::INTERRUPTED,
    }
    .into()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffx_get_last_error(
    ctx: *const LibContext,
    out_buf: *mut *mut u8,
    out_len: *mut u64,
    out_type: *mut i32,
) {
    let ctx = unsafe { &*ctx };
    let thread_id = std::thread::current().id();
    let mut guard = ctx.errors.lock().unwrap();
    if let Some(payload) = guard.remove(&thread_id) {
        match payload {
            crate::lib_context::ErrorPayload::String(s) => {
                let boxed = s.into_bytes().into_boxed_slice();
                unsafe { *out_len = boxed.len() as u64 };
                unsafe { *out_type = crate::compat::FcErrorPayloadType::STRING.into_raw() };
                unsafe { *out_buf = Box::into_raw(boxed) as *mut u8 };
            }
            crate::lib_context::ErrorPayload::Fidl(v) => {
                let boxed = v.into_boxed_slice();
                unsafe { *out_len = boxed.len() as u64 };
                unsafe { *out_type = crate::compat::FcErrorPayloadType::FIDL.into_raw() };
                unsafe { *out_buf = Box::into_raw(boxed) as *mut u8 };
            }
        }
    } else {
        unsafe { *out_len = 0 };
        unsafe { *out_type = crate::compat::FcErrorPayloadType::NONE.into_raw() };
        unsafe { *out_buf = std::ptr::null_mut() }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffx_free_error_buffer(ptr: *mut u8, len: u64) {
    if !ptr.is_null() {
        unsafe {
            let fat_ptr = std::ptr::slice_from_raw_parts_mut(ptr, len as usize);
            let _ = Box::from_raw(fat_ptr);
        }
    }
}

// LINT.ThenChange(../cpp/fuchsia_controller_internal/fuchsia_controller.h)

#[cfg(test)]
mod test {
    use super::*;
    use byteorder::{NativeEndian, ReadBytesExt};
    use fidl_fuchsia_fdomain as fdproto;
    use futures_test as _;
    use std::io::Read;
    use std::os::fd::{FromRawFd, RawFd};
    use std::os::unix::net::UnixStream;

    fn testing_lib_context() -> *const LibContext {
        let mut ctx: *const LibContext = std::ptr::null_mut();
        unsafe {
            create_ffx_lib_context(&mut ctx);
        }
        ctx
    }

    fn decode_fidl_err<T: fidl::Persistable>(lib_ctx: *const LibContext) -> T {
        let mut err_buf: *mut u8 = std::ptr::null_mut();
        let mut err_len = 0u64;
        let mut err_type = 0i32;
        unsafe { ffx_get_last_error(lib_ctx, &mut err_buf, &mut err_len, &mut err_type) };
        assert_eq!(err_type, crate::compat::FcErrorPayloadType::FIDL.into_raw()); // FIDL payload
        unsafe {
            let fat_ptr = std::ptr::slice_from_raw_parts(err_buf, err_len as usize);
            let payload = fidl::unpersist(&*fat_ptr).unwrap();
            ffx_free_error_buffer(err_buf, err_len);
            payload
        }
    }

    fn testing_env_context(lib_ctx: *const LibContext) -> *const EnvContext {
        let mut env: *const EnvContext = std::ptr::null_mut();
        unsafe {
            create_ffx_env_context(&mut env, lib_ctx, std::ptr::null(), 0, std::ptr::null());
        }
        env
    }

    fn async_channel_read(
        lib_ctx: *const LibContext,
        ch: u32,
        out_buf: &mut [u8],
        out_handle_buf: &mut [u32],
        out_buf_read: *mut u64,
        out_handle_read: *mut u64,
    ) -> FcTransportStatus {
        let fd: RawFd = unsafe { ffx_connect_handle_notifier(lib_ctx) };
        let mut notifier = unsafe { UnixStream::from_raw_fd(fd) };
        notifier.set_nonblocking(false).unwrap();
        let mut do_read = || unsafe {
            ffx_channel_read(
                lib_ctx,
                ch,
                out_buf.as_mut_ptr(),
                out_buf.len() as u64,
                out_handle_buf.as_mut_ptr(),
                out_handle_buf.len() as u64,
                out_buf_read,
                out_handle_read,
            )
        };
        let result = do_read();
        if result == FcTransportStatus::OK {
            return result;
        }
        if result != FcTransportStatus::SHOULD_WAIT {
            panic!("Unexpected channel read result: {result:?}");
        }
        let mut notifier_buf = [0u8; 4];
        let bytes_read = notifier.read(&mut notifier_buf).unwrap();
        let mut notifier_buf_reader = std::io::Cursor::new(notifier_buf);
        assert_eq!(bytes_read, 4);
        let read_handle = notifier_buf_reader.read_u32::<NativeEndian>().unwrap();
        assert_eq!(read_handle, ch, "Got notification for the wrong channel: {read_handle}");
        do_read()
    }

    fn async_socket_read(
        lib_ctx: *const LibContext,
        ch: u32,
        out_buf: &mut [u8],
        out_len: &mut u64,
    ) -> FcTransportStatus {
        let fd: RawFd = unsafe { ffx_connect_handle_notifier(lib_ctx) };
        let mut notifier = std::mem::ManuallyDrop::new(unsafe { UnixStream::from_raw_fd(fd) });
        notifier.set_nonblocking(false).unwrap();
        let mut do_read = || unsafe {
            ffx_socket_read(
                lib_ctx,
                ch,
                out_buf.as_mut_ptr(),
                out_buf.len() as u64,
                out_len as *mut u64,
            )
        };
        let result = do_read();
        if result == FcTransportStatus::OK {
            return result;
        }
        if result != FcTransportStatus::SHOULD_WAIT {
            panic!("Unexpected channel read result: {result:?}");
        }
        let mut notifier_buf = [0u8; 4];
        let bytes_read = notifier.read(&mut notifier_buf).unwrap();
        let mut notifier_buf_reader = std::io::Cursor::new(notifier_buf);
        assert_eq!(bytes_read, 4);
        let read_handle = notifier_buf_reader.read_u32::<NativeEndian>().unwrap();
        assert_eq!(read_handle, ch, "Got notification for the wrong channel: {read_handle}");
        do_read()
    }

    fn async_signal_wait(
        lib_ctx: *const LibContext,
        ch: u32,
        signals: fidl::Signals,
        signals_out: &mut fidl::Signals,
    ) -> FcTransportStatus {
        let fd: RawFd = unsafe { ffx_connect_handle_notifier(lib_ctx) };
        let mut notifier = std::mem::ManuallyDrop::new(unsafe { UnixStream::from_raw_fd(fd) });
        notifier.set_nonblocking(false).unwrap();
        let mut out: u32 = 0;
        let mut do_read = || unsafe {
            let res = ffx_object_signal_poll(lib_ctx, ch, signals.bits(), &mut out as *mut u32);
            *signals_out = fidl::Signals::from_bits_retain(out);
            return res;
        };
        let result = do_read();
        if result == FcTransportStatus::OK {
            return result;
        }
        if result != FcTransportStatus::SHOULD_WAIT {
            panic!("Unexpected channel read result: {result:?}");
        }
        let mut notifier_buf = [0u8; 4];
        let bytes_read = notifier.read(&mut notifier_buf).unwrap();
        let mut notifier_buf_reader = std::io::Cursor::new(notifier_buf);
        assert_eq!(bytes_read, 4);
        let read_handle = notifier_buf_reader.read_u32::<NativeEndian>().unwrap();
        assert_eq!(read_handle, ch, "Got notification for the wrong channel: {read_handle}");
        do_read()
    }

    #[test]
    fn channel_read_empty() {
        let lib_ctx = testing_lib_context();
        let env_ctx = testing_env_context(lib_ctx);
        let mut a: u32 = 0;
        let mut b: u32 = 0;
        let status =
            unsafe { ffx_channel_create(env_ctx, 0, &mut a as *mut u32, &mut b as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        let mut buf = [0u8; 2];
        let mut handles = [0u32; 2];
        let result = unsafe {
            ffx_channel_read(
                lib_ctx,
                a,
                buf.as_mut_ptr(),
                buf.len() as u64,
                handles.as_mut_ptr(),
                handles.len() as u64,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(result, FcTransportStatus::SHOULD_WAIT);
        unsafe {
            ffx_close_handle(lib_ctx, a);
        }
        unsafe {
            ffx_close_handle(lib_ctx, b);
        }
        unsafe { destroy_ffx_env_context(env_ctx) }
        unsafe { destroy_ffx_lib_context(lib_ctx) }
    }

    #[test]
    fn socket_read_empty() {
        let lib_ctx = testing_lib_context();
        let env_ctx = testing_env_context(lib_ctx);
        let mut a: u32 = 0;
        let mut b: u32 = 0;
        let status =
            unsafe { ffx_socket_create(env_ctx, 0, &mut a as *mut u32, &mut b as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        let mut buf = [0u8; 2];
        let result = unsafe {
            ffx_socket_read(lib_ctx, a, buf.as_mut_ptr(), buf.len() as u64, std::ptr::null_mut())
        };
        assert_eq!(result, FcTransportStatus::SHOULD_WAIT);
        unsafe {
            ffx_close_handle(lib_ctx, a);
        }
        unsafe {
            ffx_close_handle(lib_ctx, b);
        }
        unsafe { destroy_ffx_env_context(env_ctx) }
        unsafe { destroy_ffx_lib_context(lib_ctx) }
    }

    #[test]
    fn channel_read_some_data_null_out_params() {
        let lib_ctx = testing_lib_context();
        let env_ctx = testing_env_context(lib_ctx);
        let mut a: u32 = 0;
        let mut b: u32 = 0;
        let status =
            unsafe { ffx_channel_create(env_ctx, 0, &mut a as *mut u32, &mut b as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        let mut c: u32 = 0;
        let mut d: u32 = 0;
        let status =
            unsafe { ffx_channel_create(env_ctx, 0, &mut c as *mut u32, &mut d as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        let mut out_buf: [u8; 2] = [1, 2];
        let mut out_handles: [u32; 2] = [c, d];
        let status = unsafe {
            ffx_channel_write(
                lib_ctx,
                b,
                out_buf.as_mut_ptr(),
                out_buf.len() as u64,
                out_handles.as_mut_ptr(),
                out_handles.len() as u64,
            )
        };
        assert_eq!(status, FcTransportStatus::OK);
        let mut in_buf = [0u8; 2];
        let mut in_handles = [0u32; 2];
        let status = async_channel_read(
            lib_ctx,
            a,
            &mut in_buf,
            &mut in_handles,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        assert_eq!(status, FcTransportStatus::OK);
        assert_eq!(&in_buf, &[1, 2]);
        assert_eq!(&in_handles, &[c, d]);
        unsafe {
            ffx_close_handle(lib_ctx, a);
        }
        unsafe {
            ffx_close_handle(lib_ctx, b);
        }
        unsafe {
            ffx_close_handle(lib_ctx, c);
        }
        unsafe {
            ffx_close_handle(lib_ctx, d);
        }
        unsafe { destroy_ffx_env_context(env_ctx) }
        unsafe { destroy_ffx_lib_context(lib_ctx) }
    }

    #[test]
    fn channel_read_some_data_too_small_byte_buffer() {
        let lib_ctx = testing_lib_context();
        let env_ctx = testing_env_context(lib_ctx);
        let mut a: u32 = 0;
        let mut b: u32 = 0;
        let status =
            unsafe { ffx_channel_create(env_ctx, 0, &mut a as *mut u32, &mut b as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        let mut c: u32 = 0;
        let mut d: u32 = 0;
        let status =
            unsafe { ffx_channel_create(env_ctx, 0, &mut c as *mut u32, &mut d as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        let mut out_buf = [1, 2];
        let mut out_handles = [c, d];
        let status = unsafe {
            ffx_channel_write(
                lib_ctx,
                b,
                out_buf.as_mut_ptr(),
                out_buf.len() as u64,
                out_handles.as_mut_ptr(),
                out_handles.len() as u64,
            )
        };
        assert_eq!(status, FcTransportStatus::OK);
        let mut buf = [0u8; 1];
        let mut handles = [0u32; 2];
        let status = async_channel_read(
            lib_ctx,
            a,
            &mut buf,
            &mut handles,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        assert_eq!(status, FcTransportStatus::BUFFER_TOO_SMALL);
        unsafe {
            ffx_close_handle(lib_ctx, a);
        }
        unsafe {
            ffx_close_handle(lib_ctx, b);
        }
        unsafe {
            ffx_close_handle(lib_ctx, c);
        }
        unsafe {
            ffx_close_handle(lib_ctx, d);
        }
        unsafe { destroy_ffx_env_context(env_ctx) }
        unsafe { destroy_ffx_lib_context(lib_ctx) }
    }

    #[test]
    fn socket_read_some_data_too_large_byte_buffer() {
        let lib_ctx = testing_lib_context();
        let env_ctx = testing_env_context(lib_ctx);
        let mut a: u32 = 0;
        let mut b: u32 = 0;
        let status =
            unsafe { ffx_socket_create(env_ctx, 0, &mut a as *mut u32, &mut b as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        let mut out_buf = [1, 2];
        let status =
            unsafe { ffx_socket_write(lib_ctx, b, out_buf.as_mut_ptr(), out_buf.len() as u64) };
        assert_eq!(status, FcTransportStatus::OK);
        let mut buf = [0u8; 3];
        let mut bytes_len = 0u64;
        let status = async_socket_read(lib_ctx, a, &mut buf, &mut bytes_len);
        assert_eq!(status, FcTransportStatus::OK);
        assert_eq!(bytes_len, 2);
        assert_eq!(&buf[0..2], &[1, 2]);
        unsafe {
            ffx_close_handle(lib_ctx, a);
        }
        unsafe {
            ffx_close_handle(lib_ctx, b);
        }
        unsafe { destroy_ffx_env_context(env_ctx) }
        unsafe { destroy_ffx_lib_context(lib_ctx) }
    }

    #[test]
    fn channel_read_some_data_too_small_handle_buffer() {
        let lib_ctx = testing_lib_context();
        let env_ctx = testing_env_context(lib_ctx);
        let mut a: u32 = 0;
        let mut b: u32 = 0;
        let status =
            unsafe { ffx_channel_create(env_ctx, 0, &mut a as *mut u32, &mut b as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        let mut c: u32 = 0;
        let mut d: u32 = 0;
        let status =
            unsafe { ffx_channel_create(env_ctx, 0, &mut c as *mut u32, &mut d as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        let mut out_buf = [1, 2];
        let mut out_handles = [c, d];
        let status = unsafe {
            ffx_channel_write(
                lib_ctx,
                b,
                out_buf.as_mut_ptr(),
                out_buf.len() as u64,
                out_handles.as_mut_ptr(),
                out_handles.len() as u64,
            )
        };
        assert_eq!(status, FcTransportStatus::OK);
        let mut buf = [0u8; 2];
        let mut handles = [0u32; 1];
        let mut read_bytes = 0u64;
        let mut read_handles = 0u64;
        let result = async_channel_read(
            lib_ctx,
            a,
            &mut buf,
            &mut handles,
            &mut read_bytes as *mut u64,
            &mut read_handles as *mut u64,
        );
        assert_eq!(read_bytes, 2);
        assert_eq!(read_handles, 2);
        assert_eq!(result, FcTransportStatus::BUFFER_TOO_SMALL);
        unsafe {
            ffx_close_handle(lib_ctx, a);
        }
        unsafe {
            ffx_close_handle(lib_ctx, b);
        }
        unsafe {
            ffx_close_handle(lib_ctx, c);
        }
        unsafe {
            ffx_close_handle(lib_ctx, d);
        }
        unsafe { destroy_ffx_env_context(env_ctx) }
        unsafe { destroy_ffx_lib_context(lib_ctx) }
    }

    #[test]
    fn channel_read_some_data_nonnull_out_params() {
        let lib_ctx = testing_lib_context();
        let env_ctx = testing_env_context(lib_ctx);
        let mut a: u32 = 0;
        let mut b: u32 = 0;
        let status =
            unsafe { ffx_channel_create(env_ctx, 0, &mut a as *mut u32, &mut b as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        let mut c: u32 = 0;
        let mut d: u32 = 0;
        let status =
            unsafe { ffx_channel_create(env_ctx, 0, &mut c as *mut u32, &mut d as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        let mut out_buf = [1, 2];
        let mut out_handles = [c, d];
        let status = unsafe {
            ffx_channel_write(
                lib_ctx,
                b,
                out_buf.as_mut_ptr(),
                out_buf.len() as u64,
                out_handles.as_mut_ptr(),
                out_handles.len() as u64,
            )
        };
        assert_eq!(status, FcTransportStatus::OK);
        let mut buf = [0u8; 2];
        let mut handles = [0u32; 2];
        let mut read_bytes = 0u64;
        let mut read_handles = 0u64;
        let result = async_channel_read(
            lib_ctx,
            a,
            &mut buf,
            &mut handles,
            &mut read_bytes as *mut u64,
            &mut read_handles as *mut u64,
        );
        assert_eq!(result, FcTransportStatus::OK);
        assert_eq!(read_bytes, 2);
        assert_eq!(read_handles, 2);
        assert_eq!(&buf, &[1, 2]);
        assert_eq!(&handles, &[c, d]);
        unsafe {
            ffx_close_handle(lib_ctx, a);
        }
        unsafe {
            ffx_close_handle(lib_ctx, b);
        }
        unsafe {
            ffx_close_handle(lib_ctx, c);
        }
        unsafe {
            ffx_close_handle(lib_ctx, d);
        }
        unsafe { destroy_ffx_env_context(env_ctx) }
        unsafe { destroy_ffx_lib_context(lib_ctx) }
    }

    #[test]
    fn channel_write_then_read_some_data() {
        let lib_ctx = testing_lib_context();
        let env_ctx = testing_env_context(lib_ctx);
        let mut a: u32 = 0;
        let mut b: u32 = 0;
        let status =
            unsafe { ffx_channel_create(env_ctx, 0, &mut a as *mut u32, &mut b as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        let mut c: u32 = 0;
        let mut d: u32 = 0;
        let status =
            unsafe { ffx_channel_create(env_ctx, 0, &mut c as *mut u32, &mut d as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        let mut out_buf = [1, 2];
        let mut out_handles = [c, d];
        let status = unsafe {
            ffx_channel_write(
                lib_ctx,
                b,
                out_buf.as_mut_ptr(),
                out_buf.len() as u64,
                out_handles.as_mut_ptr(),
                out_handles.len() as u64,
            )
        };
        assert_eq!(status, FcTransportStatus::OK);
        let mut in_buf = [0u8; 2];
        let mut in_handles = [0u32; 2];
        let status = async_channel_read(
            lib_ctx,
            a,
            &mut in_buf,
            &mut in_handles,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        assert_eq!(status, FcTransportStatus::OK);
        assert_eq!(&in_buf, &[1, 2]);
        assert_eq!(&in_handles, &[c, d]);
        unsafe {
            ffx_close_handle(lib_ctx, a);
        }
        unsafe {
            ffx_close_handle(lib_ctx, b);
        }
        unsafe {
            ffx_close_handle(lib_ctx, c);
        }
        unsafe {
            ffx_close_handle(lib_ctx, d);
        }
        unsafe { destroy_ffx_env_context(env_ctx) }
        unsafe { destroy_ffx_lib_context(lib_ctx) }
    }

    #[test]
    fn channel_write_etc_then_read_some_data() {
        let lib_ctx = testing_lib_context();
        let env_ctx = testing_env_context(lib_ctx);
        let mut a: u32 = 0;
        let mut b: u32 = 0;
        let status =
            unsafe { ffx_channel_create(env_ctx, 0, &mut a as *mut u32, &mut b as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        let mut c: u32 = 0;
        let mut d: u32 = 0;
        let status =
            unsafe { ffx_channel_create(env_ctx, 0, &mut c as *mut u32, &mut d as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        let mut write_buf = [1u8, 2u8];
        let mut handles_buf: [zx_types::zx_handle_disposition_t; 2] = [
            zx_types::zx_handle_disposition_t {
                operation: zx_types::ZX_HANDLE_OP_MOVE,
                handle: c,
                type_: zx_types::ZX_OBJ_TYPE_CHANNEL,
                rights: zx_types::ZX_RIGHT_SAME_RIGHTS,
                result: zx_types::ZX_OK,
            },
            zx_types::zx_handle_disposition_t {
                operation: zx_types::ZX_HANDLE_OP_MOVE,
                handle: d,
                type_: zx_types::ZX_OBJ_TYPE_CHANNEL,
                rights: zx_types::ZX_RIGHT_SAME_RIGHTS,
                result: zx_types::ZX_OK,
            },
        ];
        let result = unsafe {
            ffx_channel_write_etc(
                lib_ctx,
                b,
                write_buf.as_mut_ptr(),
                2,
                handles_buf.as_mut_ptr().cast(),
                2,
            )
        };
        assert_eq!(result, FcTransportStatus::OK);
        let mut buf = [0u8; 2];
        let mut handles = [0u32; 2];
        let result = async_channel_read(
            lib_ctx,
            a,
            &mut buf,
            &mut handles,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        assert_eq!(result, FcTransportStatus::OK);
        assert_eq!(&buf, &[1, 2]);
        assert_eq!(&handles, &[c, d]);
        unsafe {
            ffx_close_handle(lib_ctx, a);
        }
        unsafe {
            ffx_close_handle(lib_ctx, b);
        }
        unsafe {
            ffx_close_handle(lib_ctx, c);
        }
        unsafe {
            ffx_close_handle(lib_ctx, d);
        }
        unsafe { destroy_ffx_env_context(env_ctx) }
        unsafe { destroy_ffx_lib_context(lib_ctx) }
    }

    #[test]
    fn channel_write_etc_unsupported_op() {
        let lib_ctx = testing_lib_context();
        let env_ctx = testing_env_context(lib_ctx);
        let mut a: u32 = 0;
        let mut b: u32 = 0;
        let status =
            unsafe { ffx_channel_create(env_ctx, 0, &mut a as *mut u32, &mut b as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        let mut c: u32 = 0;
        let mut d: u32 = 0;
        let status =
            unsafe { ffx_channel_create(env_ctx, 0, &mut c as *mut u32, &mut d as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        let mut write_buf = [1u8, 2u8];
        let mut handles_buf: [zx_types::zx_handle_disposition_t; 2] = [
            zx_types::zx_handle_disposition_t {
                operation: zx_types::ZX_HANDLE_OP_DUPLICATE,
                handle: c,
                type_: zx_types::ZX_OBJ_TYPE_CHANNEL,
                rights: zx_types::ZX_RIGHT_SAME_RIGHTS,
                result: zx_types::ZX_OK,
            },
            zx_types::zx_handle_disposition_t {
                operation: zx_types::ZX_HANDLE_OP_MOVE,
                handle: d,
                type_: zx_types::ZX_OBJ_TYPE_CHANNEL,
                rights: zx_types::ZX_RIGHT_SAME_RIGHTS,
                result: zx_types::ZX_OK,
            },
        ];
        let result = unsafe {
            ffx_channel_write_etc(
                lib_ctx,
                b,
                write_buf.as_mut_ptr(),
                2,
                handles_buf.as_mut_ptr().cast(),
                2,
            )
        };
        assert_eq!(result, FcTransportStatus::NOT_SUPPORTED);
        unsafe {
            ffx_close_handle(lib_ctx, a);
        }
        unsafe {
            ffx_close_handle(lib_ctx, b);
        }
        unsafe {
            ffx_close_handle(lib_ctx, c);
        }
        unsafe {
            ffx_close_handle(lib_ctx, d);
        }
        unsafe { destroy_ffx_env_context(env_ctx) }
        unsafe { destroy_ffx_lib_context(lib_ctx) }
    }

    #[test]
    fn channel_write_etc_invalid_arg() {
        let lib_ctx = testing_lib_context();
        let env_ctx = testing_env_context(lib_ctx);
        let mut a: u32 = 0;
        let mut b: u32 = 0;
        let status =
            unsafe { ffx_channel_create(env_ctx, 0, &mut a as *mut u32, &mut b as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        let mut c: u32 = 0;
        let mut d: u32 = 0;
        let status =
            unsafe { ffx_channel_create(env_ctx, 0, &mut c as *mut u32, &mut d as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        let mut write_buf = [1u8, 2u8];
        let mut handles_buf: [zx_types::zx_handle_disposition_t; 2] = [
            zx_types::zx_handle_disposition_t {
                operation: zx_types::ZX_HANDLE_OP_MOVE,
                handle: c,
                type_: zx_types::ZX_OBJ_TYPE_CHANNEL,
                rights: 1 << 30,
                result: zx_types::ZX_OK,
            },
            zx_types::zx_handle_disposition_t {
                operation: zx_types::ZX_HANDLE_OP_MOVE,
                handle: d,
                type_: zx_types::ZX_OBJ_TYPE_CHANNEL,
                rights: zx_types::ZX_RIGHT_SAME_RIGHTS,
                result: zx_types::ZX_OK,
            },
        ];
        let result = unsafe {
            ffx_channel_write_etc(
                lib_ctx,
                b,
                write_buf.as_mut_ptr(),
                2,
                handles_buf.as_mut_ptr().cast(),
                2,
            )
        };
        assert_eq!(result, FcTransportStatus::INVALID_ARGS);
        unsafe {
            ffx_close_handle(lib_ctx, a);
        }
        unsafe {
            ffx_close_handle(lib_ctx, b);
        }
        unsafe {
            ffx_close_handle(lib_ctx, c);
        }
        unsafe {
            ffx_close_handle(lib_ctx, d);
        }
        unsafe { destroy_ffx_env_context(env_ctx) }
        unsafe { destroy_ffx_lib_context(lib_ctx) }
    }

    #[test]
    fn socket_write_then_read_some_data() {
        let lib_ctx = testing_lib_context();
        let env_ctx = testing_env_context(lib_ctx);
        let mut a: u32 = 0;
        let mut b: u32 = 0;
        let status =
            unsafe { ffx_socket_create(env_ctx, 0, &mut a as *mut u32, &mut b as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        let mut write_buf = [1u8, 2u8];
        let result =
            unsafe { ffx_socket_write(lib_ctx, b, write_buf.as_mut_ptr(), write_buf.len() as u64) };
        assert_eq!(result, FcTransportStatus::OK);
        let mut buf = [0u8; 2];
        let mut out_len = 0;
        let result = async_socket_read(lib_ctx, a, &mut buf, &mut out_len);
        assert_eq!(result, FcTransportStatus::OK);
        assert_eq!(&buf, &[1, 2]);
        unsafe {
            ffx_close_handle(lib_ctx, a);
        }
        unsafe {
            ffx_close_handle(lib_ctx, b);
        }
        unsafe { destroy_ffx_env_context(env_ctx) }
        unsafe { destroy_ffx_lib_context(lib_ctx) }
    }

    #[test]
    fn channel_read_peer_closed() {
        let lib_ctx = testing_lib_context();
        let env_ctx = testing_env_context(lib_ctx);
        let mut a: u32 = 0;
        let mut b: u32 = 0;
        let status =
            unsafe { ffx_channel_create(env_ctx, 0, &mut a as *mut u32, &mut b as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        unsafe {
            ffx_close_handle(lib_ctx, b);
        }
        let mut buf = [0u8; 2];
        let mut handles = [0u32; 2];
        let result = async_channel_read(
            lib_ctx,
            a,
            &mut buf,
            &mut handles,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        let expected_inner = fdproto::Error::TargetError(zx_status::Status::PEER_CLOSED.into_raw());
        let expect: FcTransportStatus =
            fdomain_client::Error::FDomain(expected_inner.clone()).into();
        assert_eq!(result, expect);
        let msg = decode_fidl_err::<fdproto::Error>(lib_ctx);
        assert_eq!(msg, expected_inner);
        unsafe {
            ffx_close_handle(lib_ctx, a);
        }
        unsafe { destroy_ffx_env_context(env_ctx) }
        unsafe { destroy_ffx_lib_context(lib_ctx) }
    }

    #[test]
    fn event_pair_signal_peer_peer_closed() {
        let lib_ctx = testing_lib_context();
        let env_ctx = testing_env_context(lib_ctx);
        let mut a: u32 = 0;
        let mut b: u32 = 0;
        let status =
            unsafe { ffx_eventpair_create(env_ctx, 0, &mut a as *mut u32, &mut b as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        unsafe {
            ffx_close_handle(lib_ctx, b);
        }
        let result = unsafe {
            ffx_object_signal_peer(
                lib_ctx,
                a,
                fidl::Signals::empty().bits(),
                fidl::Signals::USER_0.bits(),
            )
        };
        let expected_inner = fdproto::Error::TargetError(zx_status::Status::PEER_CLOSED.into_raw());
        let expect: FcTransportStatus =
            fdomain_client::Error::FDomain(expected_inner.clone()).into();
        assert_eq!(result, expect.into());
        let msg = decode_fidl_err::<fdproto::Error>(lib_ctx);
        assert_eq!(msg, expected_inner);
        unsafe {
            ffx_close_handle(lib_ctx, a);
        }
        unsafe { destroy_ffx_env_context(env_ctx) }
        unsafe { destroy_ffx_lib_context(lib_ctx) }
    }

    #[test]
    fn channel_write_no_such_handle() {
        let lib_ctx = testing_lib_context();
        let env_ctx = testing_env_context(lib_ctx);
        let mut a: u32 = 0;
        let mut b: u32 = 0;
        let status =
            unsafe { ffx_channel_create(env_ctx, 0, &mut a as *mut u32, &mut b as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        unsafe {
            ffx_close_handle(lib_ctx, b);
        }
        let mut buf = [0u8; 2];
        let mut handles = [0u32; 2];
        let result = unsafe {
            ffx_channel_write(
                lib_ctx,
                a,
                buf.as_mut_ptr(),
                buf.len() as u64,
                handles.as_mut_ptr(),
                handles.len() as u64,
            )
        };
        assert_eq!(result, FcTransportStatus::INTERNAL);
        unsafe {
            ffx_close_handle(lib_ctx, a);
        }
        unsafe { destroy_ffx_env_context(env_ctx) }
        unsafe { destroy_ffx_lib_context(lib_ctx) }
    }

    #[test]
    fn socket_read_peer_closed() {
        let lib_ctx = testing_lib_context();
        let env_ctx = testing_env_context(lib_ctx);
        let mut a: u32 = 0;
        let mut b: u32 = 0;
        let status =
            unsafe { ffx_socket_create(env_ctx, 0, &mut a as *mut u32, &mut b as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        unsafe {
            ffx_close_handle(lib_ctx, b);
        }
        let mut buf = [0u8];
        let mut out_len = 0;
        let _result = async_socket_read(lib_ctx, a, &mut buf, &mut out_len);
        let result = async_socket_read(lib_ctx, a, &mut buf, &mut out_len);
        let expected_inner = fdproto::Error::TargetError(zx_status::Status::PEER_CLOSED.into_raw());
        let expect: FcTransportStatus =
            fdomain_client::Error::FDomain(expected_inner.clone()).into();
        assert_eq!(result, expect.into());
        let msg = decode_fidl_err::<fdproto::Error>(lib_ctx);
        assert_eq!(msg, expected_inner);
        unsafe {
            ffx_close_handle(lib_ctx, a);
        }
        unsafe { destroy_ffx_env_context(env_ctx) }
        unsafe { destroy_ffx_lib_context(lib_ctx) }
    }

    #[test]
    fn user_signal_events_null_out() {
        let lib_ctx = testing_lib_context();
        let env_ctx = testing_env_context(lib_ctx);
        let mut event: u32 = 0;
        let status = unsafe { ffx_event_create(env_ctx, 0, &mut event as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        let result = unsafe {
            ffx_object_signal(
                lib_ctx,
                event,
                fidl::Signals::empty().bits(),
                fidl::Signals::USER_0.bits(),
            )
        };
        assert_eq!(result, FcTransportStatus::OK);
        let result = unsafe {
            ffx_object_signal_poll(
                lib_ctx,
                event,
                fidl::Signals::USER_0.bits(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(result, FcTransportStatus::SHOULD_WAIT);
        unsafe {
            ffx_close_handle(lib_ctx, event);
        }
        unsafe { destroy_ffx_env_context(env_ctx) }
        unsafe { destroy_ffx_lib_context(lib_ctx) }
    }

    #[test]
    fn user_signal_events_one_signal() {
        let lib_ctx = testing_lib_context();
        let env_ctx = testing_env_context(lib_ctx);
        let mut event: u32 = 0;
        let status = unsafe { ffx_event_create(env_ctx, 0, &mut event as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        let result = unsafe {
            ffx_object_signal(
                lib_ctx,
                event,
                fidl::Signals::empty().bits(),
                fidl::Signals::USER_0.bits(),
            )
        };
        assert_eq!(result, FcTransportStatus::OK);
        let mut out = fidl::Signals::from_bits_retain(0);
        let result = async_signal_wait(lib_ctx, event, fidl::Signals::USER_0, &mut out);
        assert_eq!(result, FcTransportStatus::OK);
        assert_eq!(out, fidl::Signals::USER_0);
        unsafe {
            ffx_close_handle(lib_ctx, event);
        }
        unsafe { destroy_ffx_env_context(env_ctx) }
        unsafe { destroy_ffx_lib_context(lib_ctx) }
    }

    #[test]
    fn user_signal_events_many_signals() {
        let lib_ctx = testing_lib_context();
        let env_ctx = testing_env_context(lib_ctx);
        let mut event: u32 = 0;
        let status = unsafe { ffx_event_create(env_ctx, 0, &mut event as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        let result = unsafe {
            ffx_object_signal(
                lib_ctx,
                event,
                fidl::Signals::empty().bits(),
                fidl::Signals::USER_0.bits(),
            )
        };
        assert_eq!(result, FcTransportStatus::OK);
        let mut out = fidl::Signals::from_bits_retain(0);
        let signals = fidl::Signals::from_bits(
            fidl::Signals::USER_0.bits()
                | fidl::Signals::OBJECT_ALL.bits()
                | fidl::Signals::USER_2.bits(),
        )
        .unwrap();
        let result = async_signal_wait(lib_ctx, event, signals, &mut out);
        assert_eq!(result, FcTransportStatus::OK);
        assert_eq!(out, fidl::Signals::USER_0);
        unsafe {
            ffx_close_handle(lib_ctx, event);
        }
        unsafe { destroy_ffx_env_context(env_ctx) }
        unsafe { destroy_ffx_lib_context(lib_ctx) }
    }

    #[test]
    fn user_signal_event_pair() {
        let lib_ctx = testing_lib_context();
        let env_ctx = testing_env_context(lib_ctx);
        let mut event: u32 = 0;
        let mut other_event: u32 = 0;
        let status = unsafe {
            ffx_eventpair_create(env_ctx, 0, &mut event as *mut u32, &mut other_event as *mut u32)
        };
        assert_eq!(status, FcTransportStatus::OK);
        let result = unsafe {
            ffx_object_signal(
                lib_ctx,
                event,
                fidl::Signals::empty().bits(),
                fidl::Signals::USER_0.bits(),
            )
        };
        assert_eq!(result, FcTransportStatus::OK);
        let mut out = fidl::Signals::from_bits_retain(0);
        let signals = fidl::Signals::from_bits(
            fidl::Signals::USER_0.bits()
                | fidl::Signals::OBJECT_ALL.bits()
                | fidl::Signals::USER_2.bits(),
        )
        .unwrap();
        let result = async_signal_wait(lib_ctx, event, signals, &mut out);
        assert_eq!(result, FcTransportStatus::OK);
        assert_eq!(out, fidl::Signals::USER_0);
        unsafe {
            ffx_close_handle(lib_ctx, event);
        }
        unsafe {
            ffx_close_handle(lib_ctx, other_event);
        }
        unsafe { destroy_ffx_env_context(env_ctx) }
        unsafe { destroy_ffx_lib_context(lib_ctx) }
    }

    #[test]
    fn user_signal_peer_event_pair() {
        let lib_ctx = testing_lib_context();
        let env_ctx = testing_env_context(lib_ctx);
        let mut tx: u32 = 0;
        let mut rx: u32 = 0;
        let status =
            unsafe { ffx_eventpair_create(env_ctx, 0, &mut tx as *mut u32, &mut rx as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        let result = unsafe {
            ffx_object_signal_peer(
                lib_ctx,
                tx,
                fidl::Signals::empty().bits(),
                fidl::Signals::USER_0.bits(),
            )
        };
        assert_eq!(result, FcTransportStatus::OK);
        let mut out = fidl::Signals::from_bits_retain(0);
        let signals = fidl::Signals::from_bits(
            fidl::Signals::USER_0.bits()
                | fidl::Signals::OBJECT_ALL.bits()
                | fidl::Signals::USER_2.bits(),
        )
        .unwrap();
        let result = async_signal_wait(lib_ctx, rx, signals, &mut out);
        assert_eq!(result, FcTransportStatus::OK);
        assert_eq!(out, fidl::Signals::USER_0);
        unsafe {
            ffx_close_handle(lib_ctx, tx);
        }
        unsafe {
            ffx_close_handle(lib_ctx, rx);
        }
        unsafe { destroy_ffx_env_context(env_ctx) }
        unsafe { destroy_ffx_lib_context(lib_ctx) }
    }

    #[test]
    fn socket_write_peer_closed() {
        let lib_ctx = testing_lib_context();
        let env_ctx = testing_env_context(lib_ctx);
        let mut a: u32 = 0;
        let mut b: u32 = 0;
        let status =
            unsafe { ffx_socket_create(env_ctx, 0, &mut a as *mut u32, &mut b as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        unsafe {
            ffx_close_handle(lib_ctx, b);
        }
        let mut buf = [0u8];
        let result = unsafe { ffx_socket_write(lib_ctx, a, buf.as_mut_ptr(), buf.len() as u64) };
        let expected_inner = fdproto::WriteSocketError {
            error: fdproto::Error::TargetError(zx_status::Status::PEER_CLOSED.into_raw()),
            wrote: 0,
        };
        let expect: FcTransportStatus =
            fdomain_client::Error::SocketWrite(expected_inner.clone()).into();
        assert_eq!(result, expect);
        let msg = decode_fidl_err::<fdproto::WriteSocketError>(lib_ctx);
        assert_eq!(msg, expected_inner);
        unsafe {
            ffx_close_handle(lib_ctx, a);
        }
        unsafe { destroy_ffx_env_context(env_ctx) }
        unsafe { destroy_ffx_lib_context(lib_ctx) }
    }

    #[test]
    fn handle_ready_notification() {
        let lib_ctx = testing_lib_context();
        let env_ctx = testing_env_context(lib_ctx);
        let fd: RawFd = unsafe { ffx_connect_handle_notifier(lib_ctx) };
        let mut notifier = unsafe { UnixStream::from_raw_fd(fd) };
        notifier.set_nonblocking(false).unwrap();
        let mut a: u32 = 0;
        let mut b: u32 = 0;
        let status =
            unsafe { ffx_channel_create(env_ctx, 0, &mut a as *mut u32, &mut b as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        let mut buf = [0u8; 2];
        let mut handles = [0u32; 2];
        let result = unsafe {
            ffx_channel_read(
                lib_ctx,
                a,
                buf.as_mut_ptr(),
                buf.len() as u64,
                handles.as_mut_ptr(),
                handles.len() as u64,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(result, FcTransportStatus::SHOULD_WAIT);
        let mut c: u32 = 0;
        let mut d: u32 = 0;
        let status =
            unsafe { ffx_channel_create(env_ctx, 0, &mut c as *mut u32, &mut d as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        let mut write_buf = [1u8, 2u8];
        let mut handles_buf: [u32; 2] = [c, d];
        let result = unsafe {
            ffx_channel_write(
                lib_ctx,
                b,
                write_buf.as_mut_ptr(),
                2,
                handles_buf.as_mut_ptr().cast(),
                2,
            )
        };
        assert_eq!(result, FcTransportStatus::OK);
        let mut notifier_buf = [0u8; 4];
        let bytes_read = notifier.read(&mut notifier_buf).unwrap();
        let mut notifier_buf_reader = std::io::Cursor::new(notifier_buf);
        assert_eq!(bytes_read, 4);
        let read_handle = notifier_buf_reader.read_u32::<NativeEndian>().unwrap();
        assert_eq!(read_handle, a);
        unsafe {
            ffx_close_handle(lib_ctx, a);
        }
        unsafe {
            ffx_close_handle(lib_ctx, b);
        }
        unsafe {
            ffx_close_handle(lib_ctx, c);
        }
        unsafe {
            ffx_close_handle(lib_ctx, d);
        }
        unsafe { destroy_ffx_env_context(env_ctx) }
        unsafe { destroy_ffx_lib_context(lib_ctx) }
    }

    #[test]
    fn user_signal_pending() {
        let lib_ctx = testing_lib_context();
        let env_ctx = testing_env_context(lib_ctx);
        let fd: RawFd = unsafe { ffx_connect_handle_notifier(lib_ctx) };
        let mut notifier = unsafe { UnixStream::from_raw_fd(fd) };
        notifier.set_nonblocking(false).unwrap();
        let mut event: u32 = 0;
        let status = unsafe { ffx_event_create(env_ctx, 0, &mut event as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        let mut out = 0u32;
        let signals = fidl::Signals::from_bits(
            fidl::Signals::USER_0.bits()
                | fidl::Signals::OBJECT_ALL.bits()
                | fidl::Signals::USER_2.bits(),
        )
        .unwrap();
        let result = unsafe { ffx_object_signal_poll(lib_ctx, event, signals.bits(), &mut out) };
        assert_eq!(result, FcTransportStatus::SHOULD_WAIT);
        let result = unsafe {
            ffx_object_signal(
                lib_ctx,
                event,
                fidl::Signals::empty().bits(),
                fidl::Signals::USER_0.bits(),
            )
        };
        assert_eq!(result, FcTransportStatus::OK);
        let mut notifier_buf = [0u8; 4];
        let bytes_read = notifier.read(&mut notifier_buf).unwrap();
        let mut notifier_buf_reader = std::io::Cursor::new(notifier_buf);
        assert_eq!(bytes_read, 4);
        let read_handle = notifier_buf_reader.read_u32::<NativeEndian>().unwrap();
        assert_eq!(read_handle, event);
        let result = unsafe { ffx_object_signal_poll(lib_ctx, event, signals.bits(), &mut out) };
        assert_eq!(result, FcTransportStatus::OK);
        assert_eq!(out, fidl::Signals::USER_0.bits());
        unsafe {
            ffx_close_handle(lib_ctx, event);
        }
        unsafe { destroy_ffx_env_context(env_ctx) }
        unsafe { destroy_ffx_lib_context(lib_ctx) }
    }

    #[test]
    fn user_signal_peer_pending() {
        let lib_ctx = testing_lib_context();
        let env_ctx = testing_env_context(lib_ctx);
        let fd: RawFd = unsafe { ffx_connect_handle_notifier(lib_ctx) };
        let mut notifier = unsafe { UnixStream::from_raw_fd(fd) };
        notifier.set_nonblocking(false).unwrap();
        let mut tx: u32 = 0;
        let mut rx: u32 = 0;
        let status =
            unsafe { ffx_eventpair_create(env_ctx, 0, &mut tx as *mut u32, &mut rx as *mut u32) };
        assert_eq!(status, FcTransportStatus::OK);
        let mut out = 0u32;
        let signals = fidl::Signals::from_bits(
            fidl::Signals::USER_0.bits()
                | fidl::Signals::OBJECT_ALL.bits()
                | fidl::Signals::USER_2.bits(),
        )
        .unwrap();
        let result = unsafe { ffx_object_signal_poll(lib_ctx, rx, signals.bits(), &mut out) };
        assert_eq!(result, FcTransportStatus::SHOULD_WAIT);
        let result = unsafe {
            ffx_object_signal_peer(
                lib_ctx,
                tx,
                fidl::Signals::empty().bits(),
                fidl::Signals::USER_0.bits(),
            )
        };
        assert_eq!(result, FcTransportStatus::OK);
        let mut notifier_buf = [0u8; 4];
        let bytes_read = notifier.read(&mut notifier_buf).unwrap();
        let mut notifier_buf_reader = std::io::Cursor::new(notifier_buf);
        assert_eq!(bytes_read, 4);
        let read_handle = notifier_buf_reader.read_u32::<NativeEndian>().unwrap();
        assert_eq!(read_handle, rx);
        let result = unsafe { ffx_object_signal_poll(lib_ctx, rx, signals.bits(), &mut out) };
        assert_eq!(result, FcTransportStatus::OK);
        assert_eq!(out, fidl::Signals::USER_0.bits());
        unsafe {
            ffx_close_handle(lib_ctx, tx);
        }
        unsafe {
            ffx_close_handle(lib_ctx, rx);
        }
        unsafe { destroy_ffx_env_context(env_ctx) }
        unsafe { destroy_ffx_lib_context(lib_ctx) }
    }
}
