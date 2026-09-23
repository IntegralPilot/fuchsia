// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_bluetooth_hfp::CallDirection as FidlCallDirection;

/// The direction of call initiation.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Direction {
    /// Call originated on this device. This is also known as an Outgoing call.
    MobileOriginated,
    /// Call is terminated on this device. This is also known as an Incoming call.
    MobileTerminated,
}

impl From<FidlCallDirection> for Direction {
    fn from(x: FidlCallDirection) -> Self {
        match x {
            FidlCallDirection::MobileOriginated => Self::MobileOriginated,
            FidlCallDirection::MobileTerminated => Self::MobileTerminated,
        }
    }
}

impl From<Direction> for FidlCallDirection {
    fn from(x: Direction) -> Self {
        match x {
            Direction::MobileOriginated => Self::MobileOriginated,
            Direction::MobileTerminated => Self::MobileTerminated,
        }
    }
}

impl From<Direction> for i64 {
    fn from(x: Direction) -> Self {
        match x {
            Direction::MobileOriginated => 0,
            Direction::MobileTerminated => 1,
        }
    }
}
