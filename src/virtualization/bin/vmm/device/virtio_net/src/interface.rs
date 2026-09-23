// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This FFI interface is not safe to use directly as it involves passing both C++ and Rust
//! allocated objects across the interface, and callbacks may be invoked from different threads.
//! Use the guest_ethernet crate to interact with these functions.

use crate::guest_ethernet;
use zx::sys::zx_status_t;

#[repr(C)]
#[allow(non_camel_case_types)]
pub struct guest_ethernet_context_t {
    _unused: [u8; 0], // Required for FFI safety.
}

#[repr(C)]
#[allow(non_camel_case_types)]
pub struct guest_ethernet_t {
    _unused: [u8; 0], // Required for FFI safety.
}

// From libguest-ethernet.a
unsafe extern "C" {
    // Heap allocates a C++ GuestEthernetContext object, initializes the minimal driver runtime and
    // dispatchers required for GuestEthernet objects to be created. If this returns an error
    // status, the memory is reclaimed internally and calling
    // interface::guest_ethernet_context_destroy is not necessary.
    pub fn guest_ethernet_context_create(
        guest_ethernet_context_out: *mut *mut guest_ethernet_context_t,
    ) -> zx_status_t;

    // Stops the dispatchers and deletes the C++ object. This can be called from any thread.
    pub fn guest_ethernet_context_destroy(guest_ethernet_context: *mut guest_ethernet_context_t);

    // Heap allocates a C++ GuestEthernet object using a context first created by
    // guest_ethernet_context_create. Multiple GuestEthernet can and should share the same context,
    // otherwise dispatcher resources might be exhausted leading to hard-to-catch bugs. The context
    // must be kept alive for as long as the GuestEthernet object exists. If this returns an error
    // status, the memory is reclaimed internally and calling interface::guest_ethernet_destroy is
    // not necessary.
    pub fn guest_ethernet_create(
        context: *mut guest_ethernet_context_t,
        guest_ethernet_out: *mut *mut guest_ethernet_t,
    ) -> zx_status_t;

    // Deletes the C++ object. This can be called from any thread.
    pub fn guest_ethernet_destroy(guest_ethernet: *mut guest_ethernet_t);

    // Initialize the C++ GuestEthernet object by registering it with the netstack. If bridging
    // is enabled, this will temporarily drop connections to the host. Note that the C++ object
    // stores a pointer to the Rust GuestEthernet, so it must be destroyed first.
    //
    // This function is asynchronous. After calling this function, the caller must wait for the
    // C++ object to set a guest ethernet status of ZX_OK (see NetDevice::ready(..) for usage).
    pub fn guest_ethernet_initialize(
        guest_ethernet: *mut guest_ethernet_t,
        rust_guest_ethernet: *const libc::c_void,
        mac: *const u8,
        mac_len: usize,
        enable_bridge: bool,
    ) -> zx_status_t;

    // Send the given ethernet frame to the netstack. Returns ZX_OK on success, ZX_ERR_SHOULD_WAIT
    // if no buffer space is available, or other values on error.
    //
    // If ZX_ERR_SHOULD_WAIT is returned, the device should stop trying to send packets until
    // `interface::ready_for_tx` is called as they will just continue to be rejected.
    pub fn guest_ethernet_send(
        guest_ethernet: *mut guest_ethernet_t,
        data: *const u8,
        len: u16,
    ) -> zx_status_t;

    // Indicate to the netstack that a packet sent to the device via `interface::receive_rx` has
    // been written to a chain, and the memory can be reclaimed.
    pub fn guest_ethernet_complete(
        guest_ethernet: *mut guest_ethernet_t,
        buffer_id: u32,
        status: zx_status_t,
    );
}

// Set the current guest ethernet status. After initialization is complete the C++ object should
// use this to send ZX_OK. If an unrecoverable error occurs such as a disconnection from the
// netstack, an error can be sent to terminate this component.
#[unsafe(no_mangle)]
pub extern "C" fn guest_ethernet_set_status(
    guest_ethernet: *const libc::c_void,
    status: zx_status_t,
) {
    guest_ethernet::GuestEthernet::set_guest_ethernet_status(guest_ethernet, status);
}

// Notify the device that the netstack is able to receive more TX packets. Generally this is used
// to restart TX after the netstack has sent ZX_ERR_SHOULD_WAIT due to lack of buffer space.
#[unsafe(no_mangle)]
pub extern "C" fn guest_ethernet_ready_for_tx(guest_ethernet: *const libc::c_void) {
    guest_ethernet::GuestEthernet::ready_for_tx(guest_ethernet);
}

// Send the given packet to the guest once chains are available. Once this packet has been written
// to a chain, the device will call `interface::complete` with the buffer ID informing the netstack
// that the memory can be reclaimed.
#[unsafe(no_mangle)]
pub extern "C" fn guest_ethernet_receive_rx(
    guest_ethernet: *const libc::c_void,
    data: *const u8,
    len: usize,
    buffer_id: u32,
) {
    guest_ethernet::GuestEthernet::receive_rx(guest_ethernet, data, len, buffer_id);
}
