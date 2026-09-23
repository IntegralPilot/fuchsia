// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[derive(Debug, PartialEq)]
pub struct NullableHandle(u32);

impl NullableHandle {
    pub fn from_raw(raw: u32) -> Self {
        Self(raw)
    }
    pub fn as_raw(&self) -> u32 {
        self.0
    }
}

impl super::CodecHandle for NullableHandle {
    type Channel = Channel;
    fn invalid() -> Self {
        Self(0)
    }

    fn as_raw(&self) -> u32 {
        self.0
    }
}

#[derive(Debug, PartialEq)]
pub struct Channel(u32);

impl super::CodecChannel for Channel {
    type Handle = NullableHandle;
    fn is_invalid(&self) -> bool {
        self.0 == 0
    }
}

impl From<NullableHandle> for Channel {
    fn from(handle: NullableHandle) -> Self {
        Self(handle.0)
    }
}

impl From<Channel> for NullableHandle {
    fn from(channel: Channel) -> Self {
        Self(channel.0)
    }
}