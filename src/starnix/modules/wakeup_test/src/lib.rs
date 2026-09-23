// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#![recursion_limit = "256"]

//!
//! This implements a VFS device named wakeup_test. It is used as a way to expose setting a wakeup
//! timer and when it goes off, send a specified  input event to to wakeup the rest of the system.
//!
//! It is modeled after a similar approach that is implemented as a loadable custom linux kernel
//! module. Since Starnix does not support loadable kernel modules, this is implemented as a built-in
//! module, that can be optionally enabled via feature configuration.
//!
//! An example of using it would be to write a command line tool that runs in the container and
//! opens the /dev/wakeup_test device, and issues ioctls to set wakeup timers and specify the input
//! event to send when the timer goes off.
//!
//!  To set a timer to wake up the system via swiping after 2 seconds, you could run a command like this:
//!  `wakeup_test -a timers -e 2 -t cuj_tile -m swipe-right -o 7 -i 4 -u 456 -v 456`
//!
//! See go/fuchsia-wakeup-latency-test for more details.

use starnix_core::device::DeviceMode;
use starnix_core::device::kobject::DeviceMetadata;
use starnix_core::task::Kernel;
use std::sync::Arc;

use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errors::Errno;

mod device;
mod input;
mod ioctl;
mod tracing;

pub use device::WakeupTestDevice;

pub fn register_wakeup_test_device(kernel: &Arc<Kernel>) -> Result<(), Errno> {
    let registry = &kernel.device_registry;
    let misc_class = registry.objects.misc_class();
    let device = WakeupTestDevice::new(kernel);
    registry.register_device(
        kernel,
        "wakeup_test0".into(),
        DeviceMetadata::new("wakeup_test0".into(), DeviceId::new(0, 0), DeviceMode::Char),
        misc_class,
        device,
    )?;
    Ok(())
}
