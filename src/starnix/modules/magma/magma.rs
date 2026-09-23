// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(non_upper_case_globals)]

use magma::{
    MAGMA_POLL_TYPE_SEMAPHORE, magma_handle_t, magma_poll_item__bindgen_ty_1, magma_poll_item_t,
    magma_semaphore_t, virtio_magma_ctrl_hdr_t, virtio_magma_ctrl_type,
    virtmagma_ioctl_args_magma_command,
};
use starnix_core::mm::MemoryAccessorExt;
use starnix_core::task::CurrentTask;
use starnix_uapi::errors::Errno;
use starnix_uapi::user_address::{UserAddress, UserRef};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

/// Reads a magma command and its type from user space.
///
/// # Parameters
/// - `current_task`: The task to which the command memory belongs.
/// - `command_address`: The address of the `virtmagma_ioctl_args_magma_command`.
pub fn read_magma_command_and_type(
    current_task: &CurrentTask,
    command_address: UserAddress,
) -> Result<(virtmagma_ioctl_args_magma_command, virtio_magma_ctrl_type), Errno> {
    let command: virtmagma_ioctl_args_magma_command =
        current_task.read_object(UserRef::new(command_address))?;

    let request_address = UserAddress::from(command.request_address);
    let header: virtio_magma_ctrl_hdr_t =
        current_task.read_object(UserRef::new(request_address))?;

    Ok((command, header.type_ as u16))
}

/// Reads the control and response structs from the given magma command struct.
///
/// # Parameters
/// - `current_task`: The task to which the memory belongs.
/// - `command`: The command struct that contains the pointers to the control and response structs.
pub fn read_control_and_response<C: Default + IntoBytes + FromBytes, R: Default>(
    current_task: &CurrentTask,
    command: &virtmagma_ioctl_args_magma_command,
) -> Result<(C, R), Errno> {
    let request_address = UserAddress::from(command.request_address);
    let ctrl = current_task.read_object(UserRef::new(request_address))?;

    Ok((ctrl, R::default()))
}

#[repr(C)]
#[derive(IntoBytes, KnownLayout, FromBytes, Immutable, Copy, Clone, Default, Debug)]
/// `StarnixPollItem` exists to be able to `IntoBytes` and `FromBytes` the union that exists in
/// `magma_poll_item_t`.
pub struct StarnixPollItem {
    pub semaphore_or_handle: u64,
    pub type_: u32,
    pub condition: u32,
    pub result: u32,
    pub unused: u32,
}

impl StarnixPollItem {
    pub fn new(poll_item: &magma_poll_item_t) -> StarnixPollItem {
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let semaphore_or_handle = unsafe {
            if poll_item.type_ == MAGMA_POLL_TYPE_SEMAPHORE {
                poll_item.__bindgen_anon_1.semaphore
            } else {
                poll_item.__bindgen_anon_1.handle as u64
            }
        };
        StarnixPollItem {
            semaphore_or_handle,
            type_: poll_item.type_,
            condition: poll_item.condition,
            result: poll_item.result,
            unused: 0,
        }
    }

    pub fn as_poll_item(&self) -> magma_poll_item_t {
        let handle = if self.type_ == MAGMA_POLL_TYPE_SEMAPHORE {
            magma_poll_item__bindgen_ty_1 {
                semaphore: self.semaphore_or_handle as magma_semaphore_t,
            }
        } else {
            magma_poll_item__bindgen_ty_1 { handle: self.semaphore_or_handle as magma_handle_t }
        };
        magma_poll_item_t {
            __bindgen_anon_1: handle,
            type_: self.type_,
            condition: self.condition,
            result: self.result,
            unused: self.unused,
        }
    }
}
