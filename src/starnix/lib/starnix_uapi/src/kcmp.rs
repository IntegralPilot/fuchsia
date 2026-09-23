// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(non_camel_case_types)]

use crate::errors::{Errno, error};
use crate::uapi;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum KcmpResource {
    FILE,
    VM,
    FILES,
    FS,
    SIGHAND,
    IO,
    SYSVSEM,
    EPOLL_TFD,
    TYPES,
}

impl KcmpResource {
    pub fn from_raw(resource: u32) -> Result<KcmpResource, Errno> {
        Ok(match resource {
            uapi::kcmp_type_KCMP_FILE => KcmpResource::FILE,
            uapi::kcmp_type_KCMP_VM => KcmpResource::VM,
            uapi::kcmp_type_KCMP_FILES => KcmpResource::FILES,
            uapi::kcmp_type_KCMP_FS => KcmpResource::FS,
            uapi::kcmp_type_KCMP_SIGHAND => KcmpResource::SIGHAND,
            uapi::kcmp_type_KCMP_IO => KcmpResource::IO,
            uapi::kcmp_type_KCMP_SYSVSEM => KcmpResource::SYSVSEM,
            uapi::kcmp_type_KCMP_EPOLL_TFD => KcmpResource::EPOLL_TFD,
            uapi::kcmp_type_KCMP_TYPES => KcmpResource::TYPES,
            _ => return error!(EINVAL),
        })
    }
}
