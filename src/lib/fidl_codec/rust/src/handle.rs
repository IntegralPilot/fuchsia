// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub use fidl_data_zx::ObjType as ObjectType;
use fidl_data_zx::Rights;

#[cfg(feature = "classic")]
mod classic;
#[cfg(feature = "classic")]
pub use classic::*;

#[cfg(feature = "fdomain")]
mod fdomain;
#[cfg(feature = "fdomain")]
pub use fdomain::*;

#[cfg(feature = "pure")]
mod pure;
#[cfg(feature = "pure")]
pub use pure::*;
pub trait CodecHandle {
    type Channel : CodecChannel;

    fn invalid() -> Self;

    fn as_raw(&self) -> u32;
    fn is_valid(&self) -> bool {
        self.as_raw() != 0
    }
    fn is_invalid(&self) -> bool {
        !self.is_valid()
    }
}

pub trait CodecChannel {
    type Handle : CodecHandle;
    fn is_invalid(&self) -> bool;
}

pub struct HandleInfo {
    pub handle: NullableHandle,
    pub object_type: ObjectType,
    pub rights: Rights,
}

impl HandleInfo {
    pub fn new(handle: NullableHandle, object_type: ObjectType, rights: Rights) -> Self {
        Self { handle, object_type, rights }
    }
    pub fn object_type(&self) -> ObjectType {
        self.object_type
    }
    pub fn rights(&self) -> Rights {
        self.rights
    }
    pub fn into_handle(self) -> NullableHandle {
        self.handle
    }
}

#[derive(Debug, PartialEq)]
pub struct HandleDisposition {
    pub handle: NullableHandle,
    pub object_type: ObjectType,
    pub rights: Rights,
}

impl HandleDisposition {
    pub fn move_op(handle: NullableHandle, object_type: ObjectType, rights: Rights) -> Self {
        Self { handle, object_type, rights }
    }
    pub fn raw_handle(&self) -> u32 {
        self.handle.as_raw()
    }
}

pub trait AsPlatform: Sized {
    type PlatformType;
    fn as_platform(&self) -> Self::PlatformType;
}

pub trait FromPlatform<PlatformType> {
    fn from_platform(platform_type: PlatformType) -> Self;
}
