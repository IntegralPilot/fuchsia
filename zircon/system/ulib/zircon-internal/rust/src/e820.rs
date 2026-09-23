// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[repr(u32)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum E820Type {
    Ram = 1,
    Reserved = 2,
    Acpi = 3,
    Nvs = 4,
    Unusable = 5,
}

#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
pub struct E820Entry {
    pub addr: u64,
    pub size: u64,
    pub type_: E820Type,
}

const _: () = assert!(core::mem::size_of::<E820Entry>() == 20);
const _: () = assert!(core::mem::align_of::<E820Entry>() == 1);
