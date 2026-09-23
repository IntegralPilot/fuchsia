// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::completers::Completer;
use fdf::{Arena, ArenaStaticBox, fdf_arena_t};
use fidl_fuchsia_wlan_softmac as fidl_softmac;
use fuchsia_trace as trace;
use log::error;
use std::ffi::c_void;
use std::marker::{PhantomData, PhantomPinned};
use std::pin::Pin;
use std::ptr::NonNull;
use std::{mem, slice};
use wlan_fidl_ext::{TryUnpack, WithName};
use wlan_trace as wtrace;

// Defined as an opaque type as suggested by
// https://doc.rust-lang.org/nomicon/ffi.html#representing-opaque-structs.
#[repr(C)]
pub struct FfiEthernetRxCtx {
    _data: [u8; 0],
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}

#[repr(C)]
pub struct FfiEthernetRx {
    ctx: *mut FfiEthernetRxCtx,
    /// Sends an Ethernet frame to the C++ portion of wlansoftmac.
    ///
    /// # Safety
    ///
    /// Behavior is undefined unless `payload` contains a persisted `EthernetRx.Transfer` request
    /// and `payload_len` is the length of the persisted byte array.
    transfer: unsafe extern "C" fn(
        ctx: *mut FfiEthernetRxCtx,
        payload: *const u8,
        payload_len: usize,
    ) -> zx::sys::zx_status_t,
}

// Safety: The FFI provided by FfiEthernetRx is thread-safe. In particular, the wlansoftmac
// driver synchronizes all of its ddk::EthernetIfcProtocolClient calls.
unsafe impl Send for FfiEthernetRx {}

pub struct EthernetRx {
    ffi: FfiEthernetRx,
}

impl EthernetRx {
    pub fn new(ffi: FfiEthernetRx) -> Self {
        Self { ffi }
    }

    pub fn transfer(
        &mut self,
        request: &fidl_softmac::EthernetRxTransferRequest,
    ) -> Result<(), zx::Status> {
        wtrace::duration!("EthernetRx transfer");
        let payload = fidl::persist(request);
        match payload {
            Err(e) => {
                error!("Failed to persist EthernetRx.Transfer request: {}", e);
                Err(zx::Status::INTERNAL)
            }
            Ok(payload) => {
                let payload = payload.as_slice();
                // Safety: The `self.ffi.transfer` call is safe because the payload is a persisted
                // `EthernetRx.Transfer` request.
                zx::Status::ok(unsafe {
                    (self.ffi.transfer)(self.ffi.ctx, payload.as_ptr(), payload.len())
                })
            }
        }
    }
}

// Defined as an opaque type as suggested by
// https://doc.rust-lang.org/nomicon/ffi.html#representing-opaque-structs.
#[repr(C)]
pub struct FfiWlanTxCtx {
    _data: [u8; 0],
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}

#[repr(C)]
pub struct FfiWlanTx {
    ctx: *mut FfiWlanTxCtx,
    /// Sends a WLAN MAC frame to the C++ portion of wlansoftmac.
    ///
    /// # Safety
    ///
    /// Behavior is undefined unless `payload` contains a persisted `WlanTx.Transfer` request
    /// and `payload_len` is the length of the persisted byte array.
    transfer: unsafe extern "C" fn(
        ctx: *mut FfiWlanTxCtx,
        payload: *const u8,
        payload_len: usize,
    ) -> zx::sys::zx_status_t,
}

// Safety: The FFI provided by FfiWlanTx is thread-safe. In particular, the wlansoftmac
// driver synchronizes all of its fdf::SharedClient<fuchsia_wlan_softmac::WlanSoftmac>
// calls.
unsafe impl Send for FfiWlanTx {}

pub struct WlanTx {
    ffi: FfiWlanTx,
}

impl WlanTx {
    pub fn new(ffi: FfiWlanTx) -> Self {
        Self { ffi }
    }

    pub fn transfer(
        &mut self,
        request: &fidl_softmac::WlanTxTransferRequest,
    ) -> Result<(), zx::Status> {
        wtrace::duration!("WlanTx transfer");
        let payload = fidl::persist(request);
        match payload {
            Err(e) => {
                error!("Failed to persist WlanTx.Transfer request: {}", e);
                Err(zx::Status::INTERNAL)
            }
            Ok(payload) => {
                // Safety: The `self.ffi.transfer` call is safe because the payload is a persisted
                // `EthernetRx.Transfer` request.
                zx::Status::ok(unsafe {
                    (self.ffi.transfer)(self.ffi.ctx, payload.as_slice().as_ptr(), payload.len())
                })
            }
        }
    }
}

pub trait EthernetTxEventSender {
    fn unbounded_send(&self, event: EthernetTxEvent) -> Result<(), (String, EthernetTxEvent)>;
}

#[repr(C)]
pub struct FfiEthernetTxCtx {
    sender: Box<dyn EthernetTxEventSender>,
    pin: PhantomPinned,
}

#[repr(C)]
pub struct FfiEthernetTx {
    ctx: *const FfiEthernetTxCtx,
    transfer: unsafe extern "C" fn(
        ctx: *const FfiEthernetTxCtx,
        request: *const u8,
        request_size: usize,
    ) -> zx::sys::zx_status_t,
}

pub struct EthernetTx {
    ctx: Pin<Box<FfiEthernetTxCtx>>,
}

// TODO(https://fxbug.dev/42119762): We need to keep stats for these events and respond to StatsQueryRequest.
pub struct EthernetTxEvent {
    pub bytes: NonNull<[u8]>,
    pub async_id: trace::Id,
    pub borrowed_operation: Completer<Box<dyn FnOnce(zx::sys::zx_status_t)>>,
}

impl EthernetTx {
    /// Return a pinned `EthernetTx`.
    ///
    /// Pinning the returned value is imperative to ensure future `to_c_binding()` calls will return
    /// pointers that are valid for the lifetime of the returned value.
    pub fn new(sender: Box<dyn EthernetTxEventSender>) -> Self {
        Self { ctx: Box::pin(FfiEthernetTxCtx { sender, pin: PhantomPinned }) }
    }

    /// Returns a `FfiEthernetTx` containing functions to queue `EthernetTxEvent` values into the
    /// corresponding `EthernetTx`.
    ///
    /// Note that the pointers in the returned `FfiEthernetTx` are all to static and pinned values
    /// so it's safe to move this `EthernetTx` after calling this function.
    ///
    /// # Safety
    ///
    /// This method unsafe because we cannot guarantee the returned `FfiEthernetTxCtx`
    /// will have a lifetime that is shorther than this `EthernetTx`.
    ///
    /// By using this method, the caller promises the lifetime of this `EthernetTx` will exceed the
    /// `ctx` pointer used across the FFI boundary.
    pub unsafe fn to_ffi(&self) -> FfiEthernetTx {
        FfiEthernetTx {
            ctx: &*self.ctx.as_ref() as *const FfiEthernetTxCtx,
            transfer: Self::ethernet_tx_transfer,
        }
    }

    /// Queues an Ethernet frame into the `EthernetTx` for processing.
    ///
    /// The caller should either end the async
    /// trace event corresponding to |async_id| if an error occurs or deferred ending the trace to a later call
    /// into the C++ portion of wlansoftmac.
    ///
    /// Assuming no errors occur, the Rust portion of wlansoftmac will eventually
    /// rust_device_interface_t.queue_tx() with the same |async_id|. At that point, the C++ portion of
    /// wlansoftmac will assume responsibility for ending the async trace event.
    ///
    /// # Errors
    ///
    /// This function will return ZX_ERR_BAD_STATE if and only if it did not claim ownership
    /// of the eth::BorrowedOperation before returning.
    ///
    /// # Safety
    ///
    /// Behavior is undefined unless `payload` points to a persisted
    /// `fuchsia.wlan.softmac/EthernetTx.Transfer` request of length `payload_len` that is properly
    /// aligned.
    #[unsafe(no_mangle)]
    unsafe extern "C" fn ethernet_tx_transfer(
        ctx: *const FfiEthernetTxCtx,
        payload: *const u8,
        payload_len: usize,
    ) -> zx::sys::zx_status_t {
        wtrace::duration!("EthernetTx transfer");

        // Safety: This call is safe because the caller promises `payload` points to a persisted
        // `fuchsia.wlan.softmac/EthernetTx.Transfer` request of length `payload_len` that is properly
        // aligned.
        let payload = unsafe { slice::from_raw_parts(payload, payload_len) };
        let payload = match fidl::unpersist::<fidl_softmac::EthernetTxTransferRequest>(payload) {
            Ok(payload) => payload,
            Err(e) => {
                error!("Unable to unpersist EthernetTx.Transfer request: {}", e);
                return zx::Status::BAD_STATE.into_raw();
            }
        };

        let borrowed_operation =
            match payload.borrowed_operation.with_name("borrowed_operation").try_unpack() {
                Ok(x) => x as *mut c_void,
                Err(e) => {
                    let e = e.context("Missing required field in EthernetTxTransferRequest.");
                    error!("{}", e);
                    return zx::Status::BAD_STATE.into_raw();
                }
            };

        let complete_borrowed_operation: unsafe extern "C" fn(
            borrowed_operation: *mut c_void,
            status: zx::sys::zx_status_t,
        ) = match payload
            .complete_borrowed_operation
            .with_name("complete_borrowed_operation")
            .try_unpack()
        {
            // Safety: Per the safety documentation of this FFI, the sender promises
            // this field has the type unsafe extern "C" fn(*mut c_void, zx::sys::zx_status_t).
            Ok(x) => unsafe { mem::transmute(x) },
            Err(e) => {
                let e = e.context("Missing required field in EthernetTxTransferRequest.");
                error!("{}", e);
                return zx::Status::BAD_STATE.into_raw();
            }
        };

        // Box the closure so that EthernetTxEventSender can be object-safe.
        let borrowed_operation: Completer<Box<dyn FnOnce(zx::sys::zx_status_t)>> = {
            // Safety: This call of `complete_borrowed_operation` uses the value
            // of the received `borrowed_operation` field as its first argument
            // and will only be called once.
            let completer = Box::new(move |status| unsafe {
                complete_borrowed_operation(borrowed_operation, status);
            });
            // Safety: The borrowed_operation pointer and complete_borrowed_operation
            // function are both thread-safe.
            unsafe { Completer::new_unchecked(completer) }
        };

        let async_id = match payload.async_id.with_name("async_id").try_unpack() {
            Ok(x) => x,
            Err(e) => {
                let e = e.context("Missing required field in EthernetTxTransferRequest.");
                error!("{}", e);
                return zx::Status::INVALID_ARGS.into_raw();
            }
        };

        let (packet_address, packet_size) = match (
            payload.packet_address.with_name("packet_address"),
            payload.packet_size.with_name("packet_size"),
        )
            .try_unpack()
        {
            Ok(x) => x,
            Err(e) => {
                let e = e.context("Missing required field(s) in EthernetTxTransferRequest.");
                error!("{}", e);
                return zx::Status::INVALID_ARGS.into_raw();
            }
        };

        let packet_ptr = packet_address as *mut u8;
        if packet_ptr.is_null() {
            error!("EthernetTx.Transfer request contained NULL packet_address");
            return zx::Status::INVALID_ARGS.into_raw();
        }

        // Safety: This call is safe because a `EthernetTx` request is defined such that a slice
        // such as this one can be constructed from the `packet_address` and `packet_size` fields.
        let bytes = unsafe {
            NonNull::new_unchecked(slice::from_raw_parts_mut(packet_ptr, packet_size as usize))
        };

        // Safety: This dereference is safe because the lifetime of this pointer was promised to
        // live as long as function could be called when `EthernetTx::to_ffi` was called.
        match unsafe {
            (*ctx).sender.unbounded_send(EthernetTxEvent {
                bytes,
                async_id: async_id.into(),
                borrowed_operation,
            })
        } {
            Err((error, _event)) => {
                error!("Failed to queue EthernetTx.Transfer request: {}", error);
                zx::Status::INTERNAL.into_raw()
            }
            Ok(()) => zx::sys::ZX_OK,
        }
    }
}

pub trait WlanRxEventSender {
    fn unbounded_send(&self, event: WlanRxEvent) -> Result<(), (String, WlanRxEvent)>;
}

#[repr(C)]
pub struct FfiWlanRxCtx {
    sender: Box<dyn WlanRxEventSender>,
    pin: PhantomPinned,
}

#[repr(C)]
pub struct FfiWlanRx {
    ctx: *const FfiWlanRxCtx,
    transfer:
        unsafe extern "C" fn(ctx: *const FfiWlanRxCtx, request: *const u8, request_size: usize),
}

pub struct WlanRx {
    ctx: Pin<Box<FfiWlanRxCtx>>,
}

/// Indicates receipt of a MAC frame.
// TODO(https://fxbug.dev/42119762): We need to keep stats for these events and respond to StatsQueryRequest.
pub struct WlanRxEvent {
    pub bytes: ArenaStaticBox<[u8]>,
    pub rx_info: fidl_softmac::WlanRxInfo,
    pub async_id: trace::Id,
}

impl WlanRx {
    /// Return a pinned `WlanRx`.
    ///
    /// Pinning the returned value is imperative to ensure future `to_c_binding()` calls will return
    /// pointers that are valid for the lifetime of the returned value.
    pub fn new(sender: Box<dyn WlanRxEventSender>) -> Self {
        Self { ctx: Box::pin(FfiWlanRxCtx { sender, pin: PhantomPinned }) }
    }

    /// Returns a `FfiWlanRx` containing functions to queue `WlanRxEvent` values into the
    /// corresponding `WlanRx`.
    ///
    /// Note that the pointers in the returned `FfiWlanRx` are all to static and pinned values
    /// so it's safe to move this `WlanRx` after calling this function.
    ///
    /// # Safety
    ///
    /// This method unsafe because we cannot guarantee the returned `FfiWlanRxCtx`
    /// will have a lifetime that is shorther than this `WlanRx`.
    ///
    /// By using this method, the caller promises the lifetime of this `WlanRx` will exceed the
    /// `ctx` pointer used across the FFI boundary.
    pub unsafe fn to_ffi(&self) -> FfiWlanRx {
        FfiWlanRx {
            ctx: &*self.ctx.as_ref() as *const FfiWlanRxCtx,
            transfer: Self::wlan_rx_transfer,
        }
    }

    /// Queues a WLAN MAC frame into the `WlanRx` for processing.
    ///
    /// # Safety
    ///
    /// Behavior is undefined unless `payload` points to a persisted
    /// `fuchsia.wlan.softmac/WlanRx.Transfer` request of length `payload_len` that is properly
    /// aligned.
    #[unsafe(no_mangle)]
    unsafe extern "C" fn wlan_rx_transfer(
        ctx: *const FfiWlanRxCtx,
        payload: *const u8,
        payload_len: usize,
    ) {
        wtrace::duration!("WlanRx transfer");

        // Safety: This call is safe because the caller promises `payload` points to a persisted
        // `fuchsia.wlan.softmac/WlanRx.Transfer` request of length `payload_len` that is properly
        // aligned.
        let payload = unsafe { slice::from_raw_parts(payload, payload_len) };
        let payload = match fidl::unpersist::<fidl_softmac::WlanRxTransferRequest>(payload) {
            Ok(payload) => payload,
            Err(e) => {
                error!("Unable to unpersist WlanRx.Transfer request: {}", e);
                return;
            }
        };

        let async_id = match payload.async_id.with_name("async_id").try_unpack() {
            Ok(x) => x,
            Err(e) => {
                let e = e.context("Missing required field in WlanRxTransferRequest.");
                error!("{}", e);
                return;
            }
        };

        let arena = match payload.arena.with_name("arena").try_unpack() {
            Ok(x) => {
                if x == 0 {
                    error!("Received arena is null");
                    return;
                }
                // Safety: The received arena is assumed to be valid if it's not null.
                unsafe { Arena::from_raw(NonNull::new_unchecked(x as *mut fdf_arena_t)) }
            }
            Err(e) => {
                let e = e.context("Missing required field in WlanRxTransferRequest.");
                error!("{}", e);
                return;
            }
        };

        let (packet_address, packet_size, packet_info) = match (
            payload.packet_address.with_name("packet_address"),
            payload.packet_size.with_name("packet_size"),
            payload.packet_info.with_name("packet_info"),
        )
            .try_unpack()
        {
            Ok(x) => x,
            Err(e) => {
                let e = e.context("Missing required field(s) in WlanRxTransferRequest.");
                error!("{}", e);
                wtrace::async_end_wlansoftmac_rx(async_id.into(), &e.to_string());
                return;
            }
        };

        let packet_ptr = packet_address as *mut u8;
        if packet_ptr.is_null() {
            let e = "WlanRx.Transfer request contained NULL packet_address";
            error!("{}", e);
            wtrace::async_end_wlansoftmac_rx(async_id.into(), e);
            return;
        }

        // Safety: This call is safe because a `WlanRx` request is defined such that a slice
        // such as this one can be constructed from the `packet_address` and `packet_size` fields.
        // Also, the slice is allocated in `arena`.
        let bytes = unsafe {
            arena.assume_unchecked(NonNull::new_unchecked(slice::from_raw_parts_mut(
                packet_ptr,
                packet_size as usize,
            )))
        };
        let bytes = arena.make_static(bytes);

        // Safety: This dereference is safe because the lifetime of this pointer was promised to
        // live as long as function could be called when `WlanRx::to_ffi` was called.
        let _: Result<(), ()> = unsafe {
            (*ctx).sender.unbounded_send(WlanRxEvent {
                bytes,
                rx_info: packet_info,
                async_id: async_id.into(),
            })
        }
        .map_err(|(error, _event)| {
            let e = format!("Failed to queue WlanRx.Transfer request: {}", error);
            error!("{}", error);
            wtrace::async_end_wlansoftmac_rx(async_id.into(), &e);
        });
    }
}
