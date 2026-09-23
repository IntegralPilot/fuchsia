// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The guest_ethernet crate provides a safe wrapper around the guest ethernet FFI interface,
//! including appropriate memory management. See the comments in interface.rs for what each
//! FFI function does. Do not use the interface directly.

use crate::interface;
use fidl_fuchsia_net::MacAddress;

use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use std::marker::PhantomPinned;
use std::pin::Pin;

pub struct RxPacket {
    pub data: *const u8,
    pub len: usize,

    // Complete must only be called once for this ID.
    #[cfg(not(test))]
    buffer_id: u32,
    #[cfg(test)]
    pub buffer_id: u32,
}

pub struct GuestEthernetContext {
    pub context: *mut interface::guest_ethernet_context_t,
}

impl GuestEthernetContext {
    pub fn new() -> Result<Self, zx::Status> {
        let mut raw_ptr: *mut interface::guest_ethernet_context_t = std::ptr::null_mut();
        // No preconditions.
        // On success raw_ptr will contain a pointer to the context.
        // On error no cleanup or additional action is needed.
        zx::Status::ok(unsafe { interface::guest_ethernet_context_create(&mut raw_ptr) })?;

        Ok(Self { context: raw_ptr })
    }
}

impl Drop for GuestEthernetContext {
    fn drop(&mut self) {
        // Requires that self.context is either a valid pointer to a GuestEthernetContext or null.
        // As long as precondition is met the call cannot fail.
        unsafe {
            interface::guest_ethernet_context_destroy(self.context);
        }
    }
}

// A trait is used to allow mocking out the FFI wrapper when unit testing.
pub trait GuestEthernetInterface {
    fn new(context: &GuestEthernetContext) -> Result<GuestEthernetNewResult<Self>, zx::Status>
    where
        Self: Sized;
    fn initialize(&self, mac_address: MacAddress, enable_bridge: bool) -> Result<(), zx::Status>;
    fn send(&self, data: *const u8, len: u16) -> Result<(), zx::Status>;
    fn complete(&self, packet: RxPacket, status: Result<(), zx::Status>);
}

pub struct GuestEthernetNewResult<T: GuestEthernetInterface> {
    pub guest_ethernet: Pin<Box<T>>,
    pub status_rx: UnboundedReceiver<Result<(), zx::Status>>,
    pub notify_rx: UnboundedReceiver<()>,
    pub receive_packet_rx: UnboundedReceiver<RxPacket>,
}

pub struct GuestEthernet {
    raw_ptr: *mut interface::guest_ethernet_t,

    // Used by the `interface::guest_ethernet_set_status` callback.
    status_tx: UnboundedSender<Result<(), zx::Status>>,

    // Used by the `interface::guest_ethernet_ready_for_tx` callback.
    notify_tx: UnboundedSender<()>,

    // Used by the `interface::guest_ethernet_complete` callback.
    receive_packet_tx: UnboundedSender<RxPacket>,

    // GuestEthernet must not be unpinned or the C++ GuestEthernet object will interact with
    // invalid memory when calling back into Rust.
    _pin: PhantomPinned,
}

impl GuestEthernetInterface for GuestEthernet {
    fn new(
        context: &GuestEthernetContext,
    ) -> Result<GuestEthernetNewResult<GuestEthernet>, zx::Status> {
        let mut raw_ptr: *mut interface::guest_ethernet_t = std::ptr::null_mut();
        // Requires that context.context is a pointer to a fully constructed GuestEthernetContext.
        // On success raw_ptr will contain a pointer to the GuestEthernet object.
        // On error no cleanup or additional action is needed.
        zx::Status::ok(unsafe { interface::guest_ethernet_create(context.context, &mut raw_ptr) })?;

        let (status_tx, status_rx) = mpsc::unbounded::<Result<(), zx::Status>>();
        let (notify_tx, notify_rx) = mpsc::unbounded::<()>();
        let (receive_packet_tx, receive_packet_rx) = mpsc::unbounded::<RxPacket>();
        let guest_ethernet = Box::pin(Self {
            raw_ptr,
            status_tx,
            notify_tx,
            receive_packet_tx,
            _pin: PhantomPinned,
        });

        Ok(GuestEthernetNewResult { guest_ethernet, status_rx, notify_rx, receive_packet_rx })
    }

    fn initialize(&self, mac_address: MacAddress, enable_bridge: bool) -> Result<(), zx::Status> {
        log::info!("Registering a virtio-net device with the netstack");
        zx::Status::ok(unsafe {
            interface::guest_ethernet_initialize(
                self.raw_ptr,
                self as *const _ as *const libc::c_void,
                mac_address.octets.as_ptr(),
                mac_address.octets.len(),
                enable_bridge,
            )
        })
    }

    fn send(&self, data: *const u8, len: u16) -> Result<(), zx::Status> {
        zx::Status::ok(unsafe { interface::guest_ethernet_send(self.raw_ptr, data, len) })
    }

    fn complete(&self, packet: RxPacket, status: Result<(), zx::Status>) {
        let status = match status {
            Ok(()) => zx::sys::ZX_OK,
            Err(s) => s.into_raw(),
        };
        unsafe { interface::guest_ethernet_complete(self.raw_ptr, packet.buffer_id, status) }
    }
}

impl GuestEthernet {
    // Callback from C++.
    pub fn set_guest_ethernet_status(
        guest_ethernet: *const libc::c_void,
        status: zx::sys::zx_status_t,
    ) {
        let guest_ethernet = unsafe {
            (guest_ethernet as *const GuestEthernet)
                .as_ref()
                .expect("received a nullptr device from C++")
        };

        log::info!("C++ guest ethernet object sent status: {}", status);
        guest_ethernet
            .status_tx
            .unbounded_send(zx::Status::ok(status))
            .expect("status tx end should never be closed");
    }

    // Callback from C++.
    pub fn ready_for_tx(guest_ethernet: *const libc::c_void) {
        let guest_ethernet = unsafe {
            (guest_ethernet as *const GuestEthernet)
                .as_ref()
                .expect("received a nullptr device from C++")
        };

        guest_ethernet.notify_tx.unbounded_send(()).expect("notify tx end should never be closed");
    }

    // Callback from C++.
    pub fn receive_rx(
        guest_ethernet: *const libc::c_void,
        data: *const u8,
        len: usize,
        buffer_id: u32,
    ) {
        let guest_ethernet = unsafe {
            (guest_ethernet as *const GuestEthernet)
                .as_ref()
                .expect("received a nullptr device from C++")
        };

        guest_ethernet
            .receive_packet_tx
            .unbounded_send(RxPacket { data, len, buffer_id })
            .expect("receive packet tx end should never be closed");
    }
}

impl Drop for GuestEthernet {
    fn drop(&mut self) {
        unsafe {
            log::info!("Disconnecting a virtio-net device from the netstack");
            interface::guest_ethernet_destroy(self.raw_ptr);
        }
    }
}
