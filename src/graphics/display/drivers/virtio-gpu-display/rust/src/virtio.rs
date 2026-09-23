// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://fxbug.dev/504722357): Remove this in favor of more granular
// attributes when the Rust port is completed.
#![allow(unused_imports)]

//! Infrastructure for drivers targeting virtio devices.
//!
//! # Concepts
//!
//! The virtio specification describes the interactions between a driver and a
//! device. The **device** is typically a virtualized hardware model implemented
//! by an emulator, such as QEMU or crosvm. The **driver** is a component of the
//! guest operating system running under the emulator. This module provides
//! [`VirtioPciDevice`], a building block for drivers targeting virtio devices
//! using the PCI transport.
//!
//! The behavior of a virtio device depends on a set of feature bits negotiated
//! between the driver and the device. [`VirtioFeatureBits`] covers the bits
//! common to all virtio device. Drivers are recommended to encode device
//! type-specific feature bits into types that follow the same pattern as
//! [`VirtioFeatureBits`], and implement the [`From`] trait to provide
//! conversions between their types and [`VirtioFeatureBits`].
//!
//! Drivers primarily interact with devices by submitting buffers into queues.
//! One **buffer** can be modeled as a remote procedure call (RPC), in that it
//! conveys one unit of work that the device is expected to perform.
//! [`VirtioBufferRef`] describes one buffer, which is a list of memory ranges
//! modeled by [`VirtioMemoryRange`]. When the driver **submits** the buffer to
//! the device, it transfers ownership of the memory described by the buffer,
//! and asks the device to perform the work. The device **returns** the buffer
//! after it completes the work, transferring ownership of the memory back to
//! the driver.
//!
//! # References
//!
//! The documentation references the following documents.
//!
//! * [OASIS Virtual I/O Device (VIRTIO)][virtio-spec] specification - version
//!   1.4, Committee Specification 01, dated 8 April 2026, referenced as
//!   `virtio`.
//! * [PCI Local Bus Specification][pci-local-spec] - Revision 3.0, dated
//!   February 3 2004, referenced as `pci`.
//!
//! [pci-local-spec]: https://pcisig.com/PCIConventional/Specs/Base/LocalBus_3.0
//! [virtio-spec]: https://docs.oasis-open.org/virtio/virtio/v1.4/virtio-v1.4.html

mod common;
mod pci;

pub use common::device_status::DeviceStatus;
pub use common::feature_bits::VirtioFeatureBits;
pub use common::queue::buffer::{VirtioBufferRef, VirtioMemoryRange, VirtioMemoryRangeAccess};
pub use pci::device::{VirtioPciDevice, VirtioPciDeviceBuilder};
