// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ffx_symbolize::MappingDetails;
use std::collections::HashMap;
use std::fmt;
use thiserror::Error;

#[derive(Eq, Hash, PartialEq, Clone, Copy, Debug)]
pub struct Pid(pub u64);

impl fmt::Display for Pid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Eq, Hash, PartialEq, Clone, Copy, Debug)]
pub struct Tid(pub u64);

impl fmt::Display for Tid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ModuleDetails {
    pub name: String,
    pub build_id: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModuleWithMmapDetails {
    pub module: ModuleDetails,
    pub mmaps: Vec<MappingDetails>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BacktraceDetails(pub u64);

#[derive(Clone, Debug, PartialEq)]
pub struct RawSample {
    pub timestamp: u64,
    pub sample_memory: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProfilingRecordHandler {
    pub process_name: Option<String>,
    pub module_with_mmap_records: HashMap<u16, ModuleWithMmapDetails>,
    pub backtrace_records: HashMap<Tid, Vec<Vec<BacktraceDetails>>>,
    pub raw_samples: HashMap<Tid, Vec<RawSample>>,
}

#[derive(PartialEq, Debug)]
pub struct UnsymbolizedSamples {
    pub handlers: HashMap<Pid, ProfilingRecordHandler>,
    pub thread_names: HashMap<Tid, String>,
}

#[derive(Error, Debug)]
pub enum SymbolizeError {
    #[error("Failed to load ffx environment context.")]
    NoFfxEnvironmentContext,

    #[error("Failed to open the profiler file due to {}", .0)]
    FileError(#[from] std::io::Error),

    #[error("Failed to create symbolizer due to {}", .0)]
    SymbolizerError(#[from] ffx_symbolize::CreateSymbolizerError),

    #[error("Failed to add mapping due to {}", .0)]
    AddMappingError(#[from] ffx_symbolize::AddMappingError),

    #[error("Failed to convert string to u64 due to {}", .0)]
    HexConvertError(#[from] hex::FromHexError),

    #[error("Encountered an unsupported FXT record type.")]
    UnsupportedFxtRecord,

    #[error("Failed to parse FXT file: {}", .0)]
    FxtParseError(#[from] fxt::ParseError),

    #[error("Received non-profiler FXT record.")]
    NonProfilerFxtRecord,

    #[error("Invalid mapping record.")]
    InvalidMappingRecord,
}
