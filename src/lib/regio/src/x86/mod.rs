// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod cpuid;
mod cr;
mod msr;
mod pio;

pub use cpuid::{Cpuid, CpuidRawResult, CpuidValue, DirectCpuid};
pub use cr::{Cr, CrIo, Xcr, XcrIo};
pub use msr::{Msr, MsrIo};
pub use pio::{Port, PortIo};

/// Represents the EAX register.
pub const EAX: u8 = 0;

/// Represents the EBX register.
pub const EBX: u8 = 1;

/// Represents the ECX register.
pub const ECX: u8 = 2;

/// Represents the EDX register.
pub const EDX: u8 = 3;
