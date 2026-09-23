// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
// Keep all consts and type defs for completeness.
#![allow(dead_code)]

use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

pub use zerocopy::byteorder::little_endian::{U16 as LE16, U64 as LE64};

// 5.15.1 Virtqueues
//
pub const GUESTREQUESTQ: u16 = 0;

// 5.15.6 Device Operation
pub const VIRTIO_MEM_REQ_PLUG: u16 = 0;
pub const VIRTIO_MEM_REQ_UNPLUG: u16 = 1;
pub const VIRTIO_MEM_REQ_UNPLUG_ALL: u16 = 2;
pub const VIRTIO_MEM_REQ_STATE: u16 = 3;

#[derive(Debug, Default, Copy, Clone, IntoBytes, KnownLayout, FromBytes, Immutable)]
#[repr(C, packed)]
pub struct VirtioMemRequest {
    pub ty: LE16,
    pub _padding: [LE16; 3],
    // payload is identical for PLUG, UNPLUG, UNPLUG_ALL and STATE
    pub addr: LE64,
    pub nb_blocks: LE16,
    pub _payload_padding: [LE16; 3],
}

pub const VIRTIO_MEM_RESP_ACK: u16 = 0;
pub const VIRTIO_MEM_RESP_NACK: u16 = 1;
pub const VIRTIO_MEM_RESP_BUSY: u16 = 2;
pub const VIRTIO_MEM_RESP_ERROR: u16 = 3;

pub const VIRTIO_MEM_STATE_PLUGGED: u16 = 0;
pub const VIRTIO_MEM_STATE_UNPLUGGED: u16 = 1;
pub const VIRTIO_MEM_STATE_MIXED: u16 = 2;

#[derive(Debug, Default, Copy, Clone, IntoBytes, KnownLayout, FromBytes, Immutable)]
#[repr(C, packed)]
pub struct VirtioMemResponse {
    pub ty: LE16,
    pub _padding: [LE16; 3],
    // payload is the same for all responses
    pub state: LE16,
}
