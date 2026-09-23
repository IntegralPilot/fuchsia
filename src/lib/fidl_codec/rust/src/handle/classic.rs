// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{AsPlatform, FromPlatform};

pub use fidl::{Channel, NullableHandle};

impl super::CodecHandle for NullableHandle {
    type Channel = Channel;
    fn invalid() -> Self {
        Self::invalid()
    }

    fn as_raw(&self) -> u32 {
        self.as_handle_ref().raw_handle()
    }
}

impl super::CodecChannel for Channel {
    type Handle = NullableHandle;
    fn is_invalid(&self) -> bool {
        self.as_handle_ref().is_invalid()
    }
}

impl AsPlatform for super::Rights {
    type PlatformType = fidl::Rights;
    fn as_platform(&self) -> Self::PlatformType {
        fidl::Rights::from_bits_retain(self.bits())
    }
}

impl FromPlatform<fidl::Rights> for super::Rights {
    fn from_platform(platform_type: fidl::Rights) -> Self {
        Self::from_bits_retain(platform_type.bits())
    }
}

impl AsPlatform for super::ObjectType {
    type PlatformType = fidl::ObjectType;
    fn as_platform(&self) -> Self::PlatformType {
        fidl::ObjectType::from_raw(*self as u32)
    }
}

impl FromPlatform<fidl::ObjectType> for super::ObjectType {
    fn from_platform(platform_type: fidl::ObjectType) -> Self {
        Self::from_raw(platform_type.into_raw()).unwrap_or(Self::None)
    }
}

impl From<fidl::HandleInfo> for super::HandleInfo {
    fn from(handle_info: fidl::HandleInfo) -> Self {
        Self {
            handle: handle_info.handle.into(),
            object_type: super::ObjectType::from_platform(handle_info.object_type),
            rights: super::Rights::from_platform(handle_info.rights),
        }
    }
}

impl<'a> Into<fidl::HandleDisposition<'a>> for super::HandleDisposition {
    fn into(self) -> fidl::HandleDisposition<'a> {
        fidl::HandleDisposition::new(
            fidl::HandleOp::Move(self.handle.into()),
            self.object_type.as_platform(),
            self.rights.as_platform(),
            Ok(()),
        )
    }
}
