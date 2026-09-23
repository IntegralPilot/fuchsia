// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(target_os = "fuchsia")]
pub mod collector;

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct SymbolizationTestOutputs {
    pub libc_addr: u64,
    pub fn_one_addr: u64,
    pub fn_two_addr: u64,
    pub fn_sys_inc_addr: u64,
    pub heap_addr: u64,
    pub no_symbol_addr: u64,
    pub fn_one_source_line: u32,
    pub fn_two_source_line: u32,
    pub modules: Vec<Module>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Module {
    pub name: String,
    pub build_id: Vec<u8>,
    pub mappings: Vec<Mapping>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Mapping {
    pub start_addr: u64,
    pub size: u64,
    pub vaddr: u64,
    pub readable: bool,
    pub writeable: bool,
    pub executable: bool,
}
