// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::uapi;
use bitflags::bitflags;

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct InotifyMask: u32 {
        // Events
        const ACCESS = uapi::IN_ACCESS;
        const MODIFY = uapi::IN_MODIFY;
        const ATTRIB = uapi::IN_ATTRIB;
        const CLOSE_WRITE = uapi::IN_CLOSE_WRITE;
        const CLOSE_NOWRITE = uapi::IN_CLOSE_NOWRITE;
        const OPEN = uapi::IN_OPEN;
        const MOVE_FROM = uapi::IN_MOVED_FROM;
        const MOVE_TO = uapi::IN_MOVED_TO;
        const CREATE = uapi::IN_CREATE;
        const DELETE = uapi::IN_DELETE;
        const DELETE_SELF = uapi::IN_DELETE_SELF;
        const MOVE_SELF = uapi::IN_MOVE_SELF;

        // Special events
        const UNMOUNT = uapi::IN_UNMOUNT;
        const Q_OVERFLOW = uapi::IN_Q_OVERFLOW;
        const IGNORED = uapi::IN_IGNORED;

        // Multi-bit flags
        const CLOSE = uapi::IN_CLOSE;
        const MOVE = uapi::IN_MOVE;
        const ALL_EVENTS = uapi::IN_ALL_EVENTS;

        const ONLYDIR = uapi::IN_ONLYDIR;
        const DONT_FOLLOW = uapi::IN_DONT_FOLLOW;
        const EXCL_UNLINK = uapi::IN_EXCL_UNLINK;
        const MASK_CREATE = uapi::IN_MASK_CREATE;
        const MASK_ADD = uapi::IN_MASK_ADD;
        const ISDIR = uapi::IN_ISDIR;
        const ONESHOT = uapi::IN_ONESHOT;
        const CLOEXEC = uapi::IN_CLOEXEC;
        const NONBLOCK = uapi::IN_NONBLOCK;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::fuchsia::test]
    fn multi_bit_flags() {
        assert_eq!(InotifyMask::CLOSE, InotifyMask::CLOSE_WRITE | InotifyMask::CLOSE_NOWRITE);
        assert_eq!(InotifyMask::MOVE, InotifyMask::MOVE_FROM | InotifyMask::MOVE_TO);
        assert_eq!(
            InotifyMask::ALL_EVENTS,
            InotifyMask::ACCESS
                | InotifyMask::MODIFY
                | InotifyMask::ATTRIB
                | InotifyMask::CLOSE_WRITE
                | InotifyMask::CLOSE_NOWRITE
                | InotifyMask::OPEN
                | InotifyMask::MOVE_FROM
                | InotifyMask::MOVE_TO
                | InotifyMask::CREATE
                | InotifyMask::DELETE
                | InotifyMask::DELETE_SELF
                | InotifyMask::MOVE_SELF
        );
    }
}
