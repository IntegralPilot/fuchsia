// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use linux_uapi::{_IOC_NRMASK, _IOC_NRSHIFT};
use zerocopy::FromBytes;

#[repr(i32)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum WakeupMethod {
    PowerButton = 0,
    WakeupByTouch,
    WakeupBySwipeUp,
    WakeupBySwipeDown,
    WakeupBySwipeLeft,
    WakeupBySwipeRight,
}

impl From<i32> for WakeupMethod {
    fn from(value: i32) -> Self {
        match value {
            0 => Self::PowerButton,
            1 => Self::WakeupByTouch,
            2 => Self::WakeupBySwipeUp,
            3 => Self::WakeupBySwipeDown,
            4 => Self::WakeupBySwipeLeft,
            5 => Self::WakeupBySwipeRight,
            _ => Self::PowerButton,
        }
    }
}
impl Default for WakeupMethod {
    fn default() -> Self {
        Self::PowerButton
    }
}

impl Into<&'static str> for WakeupMethod {
    fn into(self) -> &'static str {
        match self {
            WakeupMethod::PowerButton => "WakeupByPowerButton",
            WakeupMethod::WakeupByTouch => "WakeupByTouch",
            WakeupMethod::WakeupBySwipeUp => "WakeupBySwipeUp",
            WakeupMethod::WakeupBySwipeDown => "WakeupBySwipeDown",
            WakeupMethod::WakeupBySwipeLeft => "WakeupBySwipeLeft",
            WakeupMethod::WakeupBySwipeRight => "WakeupBySwipeRight",
        }
    }
}

/*
 * Different test types started from user space
 */
#[repr(i32)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum WakeupTestType {
    TestRegularWakeup = 0,
    TestCujSwipeQss,
    TestCujSwipeTile,
    TestCujSwipeNotification,
    TestCujTapToApp,
}

impl From<u32> for WakeupTestType {
    fn from(value: u32) -> Self {
        match value {
            0 => Self::TestRegularWakeup,
            1 => Self::TestCujSwipeQss,
            2 => Self::TestCujSwipeTile,
            3 => Self::TestCujSwipeNotification,
            4 => Self::TestCujTapToApp,
            _ => Self::TestRegularWakeup,
        }
    }
}
impl Default for WakeupTestType {
    fn default() -> Self {
        Self::TestRegularWakeup
    }
}
impl Into<&'static str> for WakeupTestType {
    fn into(self) -> &'static str {
        match self {
            Self::TestRegularWakeup => "TestRegularWakeup",
            Self::TestCujSwipeQss => "TestCujSwipeQss",
            Self::TestCujSwipeTile => "TestCujSwipeTile",
            Self::TestCujSwipeNotification => "TestCujSwipeNotification",
            Self::TestCujTapToApp => "TestCujTapToApp",
        }
    }
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone, FromBytes)]
pub struct WakeupTimerInfo {
    pub version: i64,
    pub test_type: u32,
    pub interval: i64,
    pub offset: i64,
    pub method: i32,
    pub num_events: u32,
    pub coord_x: u32,
    pub coord_y: u32,
    pub dev_option: u32,
    pub screen_x: u32,
    pub screen_y: u32,
}

#[derive(Debug)]
pub enum CommandCode {
    WakeupSetTimers = 1,
    WakeupHowManyTimers,
    WakeupCancelTimers,
    WakeupTest,
    Unknown,
}

impl From<u32> for CommandCode {
    fn from(value: u32) -> Self {
        match (value & _IOC_NRMASK) >> _IOC_NRSHIFT {
            1 => Self::WakeupSetTimers,
            2 => Self::WakeupHowManyTimers,
            3 => Self::WakeupCancelTimers,
            4 => Self::WakeupTest,
            _ => Self::Unknown,
        }
    }
}
