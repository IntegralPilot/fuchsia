// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Type-safe bindings for Zircon MSI objects.

use crate::NullableHandle;

/// An object representing a Zircon Message Signaled Interrupt (MSI).
///
/// As essentially a subtype of `NullableHandle`, it can be freely interconverted.
#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct Msi(NullableHandle);
impl_handle_based!(Msi);
