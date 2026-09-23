// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error;
use crate::errors::Errno;

// See "The symbolic names are defined in the kernel source, but are
// not exported to user space; you will either need to use the numbers,
// or define the names yourself" at syslog(2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyslogAction {
    Close = 0,
    Open = 1,
    Read = 2,
    ReadAll = 3,
    ReadClear = 4,
    Clear = 5,
    ConsoleOff = 6,
    ConsoleOn = 7,
    ConsoleLevel = 8,
    SizeUnread = 9,
    SizeBuffer = 10,
}

impl TryFrom<i32> for SyslogAction {
    type Error = Errno;
    fn try_from(number: i32) -> Result<SyslogAction, Self::Error> {
        match number {
            0 => Ok(Self::Close),
            1 => Ok(Self::Open),
            2 => Ok(Self::Read),
            3 => Ok(Self::ReadAll),
            4 => Ok(Self::ReadClear),
            5 => Ok(Self::Clear),
            6 => Ok(Self::ConsoleOff),
            7 => Ok(Self::ConsoleOn),
            8 => Ok(Self::ConsoleLevel),
            9 => Ok(Self::SizeUnread),
            10 => Ok(Self::SizeBuffer),
            _ => return error!(EINVAL),
        }
    }
}
