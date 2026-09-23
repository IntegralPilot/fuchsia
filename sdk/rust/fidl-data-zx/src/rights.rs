// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// DO NOT EDIT.
// Generated from FIDL library `zx` by zither, a Fuchsia platform tool.

use bitflags::bitflags;
use zerocopy::{FromBytes, Immutable, IntoBytes};

#[repr(C)]
#[derive(
    Immutable, IntoBytes, FromBytes, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub struct Rights(u32);

bitflags! {
    impl Rights : u32 {
        const DUPLICATE = 1 << 0;
        const TRANSFER = 1 << 1;
        const READ = 1 << 2;
        const WRITE = 1 << 3;
        const EXECUTE = 1 << 4;
        const MAP = 1 << 5;
        const GET_PROPERTY = 1 << 6;
        const SET_PROPERTY = 1 << 7;
        const ENUMERATE = 1 << 8;
        const DESTROY = 1 << 9;
        const SET_POLICY = 1 << 10;
        const GET_POLICY = 1 << 11;
        const SIGNAL = 1 << 12;
        const SIGNAL_PEER = 1 << 13;
        const WAIT = 1 << 14;
        const INSPECT = 1 << 15;
        const MANAGE_JOB = 1 << 16;
        const MANAGE_PROCESS = 1 << 17;
        const MANAGE_THREAD = 1 << 18;
        const APPLY_PROFILE = 1 << 19;
        const MANAGE_SOCKET = 1 << 20;
        const OP_CHILDREN = 1 << 21;
        const RESIZE = 1 << 22;
        const ATTACH_VMO = 1 << 23;
        const MANAGE_VMO = 1 << 24;
        const SAME_RIGHTS = 1 << 31;
  }
}

pub const RIGHTS_BASIC: Rights = Rights::from_bits_truncate(0b1100000000000011); // Rights.TRANSFER | Rights.DUPLICATE | Rights.WAIT | Rights.INSPECT

pub const RIGHTS_IO: Rights = Rights::from_bits_truncate(0b1100); // Rights.READ | Rights.WRITE

pub const RIGHTS_PROPERTY: Rights = Rights::from_bits_truncate(0b11000000); // Rights.GET_PROPERTY | Rights.SET_PROPERTY

pub const RIGHTS_POLICY: Rights = Rights::from_bits_truncate(0b110000000000); // Rights.GET_POLICY | Rights.SET_POLICY

pub const DEFAULT_CHANNEL_RIGHTS: Rights = Rights::from_bits_truncate(0b1111000000001110); // Rights.TRANSFER | Rights.WAIT | Rights.INSPECT | RIGHTS_IO | Rights.SIGNAL | Rights.SIGNAL_PEER

pub const DEFAULT_EVENT_RIGHTS: Rights = Rights::from_bits_truncate(0b1101000000000011); // RIGHTS_BASIC | Rights.SIGNAL
