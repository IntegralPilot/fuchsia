// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdomain_client::AsHandleRef;

use crate::{AsPlatform, FromPlatform};

pub use fdomain_client::{NullableHandle, Channel};

impl super::CodecHandle for NullableHandle {
    type Channel = Channel;
    fn invalid() -> Self {
        Self::invalid()
    }

    fn as_raw(&self) -> u32 {
        self.as_handle_ref().id()
    }
}

impl super::CodecChannel for Channel {
    type Handle = NullableHandle;
    fn is_invalid(&self) -> bool {
        self.as_handle_ref().id() == 0
    }
}


impl<'a> Into<fdomain_client::HandleOp<'a>> for super::HandleDisposition {
    fn into(self) -> fdomain_client::HandleOp<'a> {
        fdomain_client::HandleOp::Move(self.handle.into(), self.rights.as_platform())
    }
}

impl From<fdomain_client::HandleInfo> for super::HandleInfo {
    fn from(info: fdomain_client::HandleInfo) -> Self {
        let object_type = FromPlatform::from_platform(info.handle.object_type());
        Self {
            handle: info.handle.into(),
            object_type,
            rights: super::FromPlatform::from_platform(info.rights),
        }
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
        match self {
            super::ObjectType::Channel => fidl::ObjectType::CHANNEL,
            super::ObjectType::Socket => fidl::ObjectType::SOCKET,
            _ => todo!(),
        }
    }
}

impl super::FromPlatform<fidl::ObjectType> for super::ObjectType {
    fn from_platform(platform_type: fidl::ObjectType) -> Self {
        match platform_type {
            fidl::ObjectType::CHANNEL => super::ObjectType::Channel,
            fidl::ObjectType::SOCKET => super::ObjectType::Socket,
            _ => todo!(),
        }
    }
}
