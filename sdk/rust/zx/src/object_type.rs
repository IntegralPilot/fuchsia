// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::sys;

/// Zircon object types.
///
/// # Layout
///
/// This type is guaranteed to have the same layout and bit patterns as `zx_obj_type_t`.
#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Hash)]
#[repr(transparent)]
pub struct ObjectType(sys::zx_obj_type_t);

assoc_values!(ObjectType, [
    NONE            = sys::ZX_OBJ_TYPE_NONE;
    PROCESS         = sys::ZX_OBJ_TYPE_PROCESS;
    THREAD          = sys::ZX_OBJ_TYPE_THREAD;
    VMO             = sys::ZX_OBJ_TYPE_VMO;
    CHANNEL         = sys::ZX_OBJ_TYPE_CHANNEL;
    EVENT           = sys::ZX_OBJ_TYPE_EVENT;
    PORT            = sys::ZX_OBJ_TYPE_PORT;
    INTERRUPT       = sys::ZX_OBJ_TYPE_INTERRUPT;
    PCI_DEVICE      = sys::ZX_OBJ_TYPE_PCI_DEVICE;
    DEBUGLOG        = sys::ZX_OBJ_TYPE_DEBUGLOG;
    SOCKET          = sys::ZX_OBJ_TYPE_SOCKET;
    RESOURCE        = sys::ZX_OBJ_TYPE_RESOURCE;
    EVENTPAIR       = sys::ZX_OBJ_TYPE_EVENTPAIR;
    JOB             = sys::ZX_OBJ_TYPE_JOB;
    VMAR            = sys::ZX_OBJ_TYPE_VMAR;
    FIFO            = sys::ZX_OBJ_TYPE_FIFO;
    GUEST           = sys::ZX_OBJ_TYPE_GUEST;
    VCPU            = sys::ZX_OBJ_TYPE_VCPU;
    TIMER           = sys::ZX_OBJ_TYPE_TIMER;
    IOMMU           = sys::ZX_OBJ_TYPE_IOMMU;
    BTI             = sys::ZX_OBJ_TYPE_BTI;
    PROFILE         = sys::ZX_OBJ_TYPE_PROFILE;
    PMT             = sys::ZX_OBJ_TYPE_PMT;
    SUSPEND_TOKEN   = sys::ZX_OBJ_TYPE_SUSPEND_TOKEN;
    PAGER           = sys::ZX_OBJ_TYPE_PAGER;
    EXCEPTION       = sys::ZX_OBJ_TYPE_EXCEPTION;
    CLOCK           = sys::ZX_OBJ_TYPE_CLOCK;
    STREAM          = sys::ZX_OBJ_TYPE_STREAM;
    MSI             = sys::ZX_OBJ_TYPE_MSI;
    IOB             = sys::ZX_OBJ_TYPE_IOB;
    COUNTER         = sys::ZX_OBJ_TYPE_COUNTER;
]);

impl ObjectType {
    /// Creates an `ObjectType` from the underlying zircon type.
    pub const fn from_raw(raw: sys::zx_obj_type_t) -> Self {
        Self(raw)
    }

    /// Converts `ObjectType` into the underlying zircon type.
    pub const fn into_raw(self) -> sys::zx_obj_type_t {
        self.0
    }
}
