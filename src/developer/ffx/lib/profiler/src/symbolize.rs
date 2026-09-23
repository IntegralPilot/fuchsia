// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub use crate::parse::{SymbolizeError, UnsymbolizedSamples};

use crate::parse::{BacktraceDetails, ModuleWithMmapDetails, Pid, ProfilingRecordHandler, Tid};
use ffx_symbolize::{ResolvedLocation, Symbolizer};
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::Path;

// It defines how many processes a symbolizer will handle.
// We create a symbolizer for every thread.
// More threads => more symbolizers => more latency, but higher throughput.
// NUM_PROCESS_PER_THREAD is a hard coded number considering the trade off above.
const NUM_PROCESS_PER_THREAD: usize = 4;

/// A resolved address.
#[derive(Clone, PartialEq)]
pub struct ResolvedAddress {
    /// Address for which source locations were resolved.
    pub addr: u64,
    /// Source locations found at `addr`.
    pub locations: Vec<ResolvedLocation>,
}

impl std::fmt::Debug for ResolvedAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedAddress")
            .field("addr", &format_args!("0x{:x}", self.addr))
            .field("lines", &self.locations)
            .finish()
    }
}

/// Symbolized record hash map. key: pid, value: all of the records belong to this pid.
#[derive(Clone, Debug, Default)]
pub struct SymbolizedRecords {
    pub records: Vec<(Pid, Option<String>, Vec<SymbolizedRecord>)>,
}

/// Symbolized bt list for a single tid.
#[derive(Clone, Debug)]
pub struct SymbolizedRecord {
    pub tid: Tid,
    pub thread_name: Option<String>,
    pub call_stacks: Vec<Vec<ResolvedAddress>>,
}

impl SymbolizedRecord {
    fn add_backtraces(&mut self, backtraces: Vec<ResolvedAddress>) {
        self.call_stacks.push(backtraces);
    }
}

pub fn create_unsymbolized_samples(
    input: impl AsRef<Path>,
) -> Result<UnsymbolizedSamples, SymbolizeError> {
    logging_rust_cpp_bridge::init_with_log_severity(logging_rust_cpp_bridge::FUCHSIA_LOG_FATAL);
    UnsymbolizedSamples::new_from_fxt_file(input)
}

fn find_debug_file(
    symbol_index: &symbol_index::SymbolIndex,
    binary_id: &str,
) -> Option<std::path::PathBuf> {
    if binary_id.len() <= 2 {
        return None;
    }
    if let Some(p) = symbol_index.build_id_dirs.iter().find_map(|dir| {
        let p = std::path::PathBuf::from(&dir.path)
            .join(&binary_id[..2])
            .join(format!("{}.debug", &binary_id[2..]));
        p.exists().then_some(p)
    }) {
        return Some(p);
    }

    // Fallback to the default symbol cache directory
    if let Ok(home) = std::env::var("HOME") {
        let p = std::path::PathBuf::from(home)
            .join(".fuchsia/debug/symbol-cache")
            .join(&binary_id[..2])
            .join(format!("{}.debug", &binary_id[2..]));
        if p.exists() {
            return Some(p);
        }
    }
    None
}

impl UnsymbolizedSamples {
    pub fn process_unsymbolized_samples(
        self,
        output: impl AsRef<Path>,
        pprof_conversion: bool,
        context: &ffx_config::EnvironmentContext,
    ) -> Result<SymbolizedRecords, SymbolizeError> {
        let symbol_index_path =
            symbol_index::global_symbol_index_path().unwrap_or_else(|_| "".to_string());
        let global_symbol_index = symbol_index::SymbolIndex::load_aggregate(&symbol_index_path)
            .unwrap_or_else(|_| symbol_index::SymbolIndex::new());

        let handlers: Vec<(Pid, ProfilingRecordHandler)> = self.handlers.into_iter().collect();
        let symbolized_samples = handlers.par_iter().chunks(NUM_PROCESS_PER_THREAD).map(|chunk| -> Result<Vec<(Pid, Option<String>, Vec<SymbolizedRecord>)>, SymbolizeError> {
        let mut symbolizer = Symbolizer::with_context(context)?;
        let symbolized_samples_per_thread: Result<Vec<(Pid, Option<String>, Vec<SymbolizedRecord>)>, SymbolizeError> = chunk.into_iter().map(|(pid, handler):&(Pid, ProfilingRecordHandler)| -> Result<(Pid, Option<String>, Vec<SymbolizedRecord>), SymbolizeError> {
                    let mut res_per_pid = vec![];

                    let unwinder = crate::unwinder::Unwinder::new();

                    // We use a hashmap to store the seen backtrace, to avoid symbolize the same backtrace multiple times.
                    let mut seen_bt: HashMap<BacktraceDetails, ResolvedAddress> = HashMap::new();
                    for ModuleWithMmapDetails {module, mmaps} in handler.module_with_mmap_records.values() {
                        let build_id_bytes = hex::decode(&module.build_id)?;
                        let module_id = symbolizer
                            .add_module(&module.name, &build_id_bytes);

                        // Provide the unstripped host binary file path to the unwinder
                        let debug_file = find_debug_file(&global_symbol_index, &module.build_id);
                        let debug_path = debug_file.as_ref().and_then(|p| p.to_str());

                        let mut min_load_address = u64::MAX;
                        for mmap_record in mmaps {
                            symbolizer.add_mapping(module_id, mmap_record.clone())?;
                            if debug_path.is_some() {
                                let load_address = mmap_record.start_addr.saturating_sub(mmap_record.vaddr);
                                min_load_address = min_load_address.min(load_address);
                            }
                        }
                        if let Some(path_str) = debug_path {
                            if min_load_address != u64::MAX {
                                unwinder.add_module(min_load_address, path_str);
                            }
                        }
                    }

                    for (tid, backtraces) in &handler.backtrace_records {
                        let thread_name = self.thread_names.get(tid).cloned();
                        let mut symbolized_record = SymbolizedRecord {
                            tid: *tid,
                            thread_name,
                            call_stacks: Vec::new(),
                        };

                        for call_stack in backtraces {
                            let mut current_call_stack = vec![];
                            for backtrace in call_stack {
                                let resolved_addr =
                                    seen_bt.entry(*backtrace).or_insert_with_key(|bt_key| {
                                        let resolved_locations = symbolizer
                                            .resolve_addr(bt_key.0)
                                            .unwrap_or_default();
                                        ResolvedAddress {
                                            addr: bt_key.0,
                                            locations: resolved_locations,
                                        }
                                    }).to_owned();
                                current_call_stack.push(resolved_addr);
                            }
                            symbolized_record.add_backtraces(current_call_stack);
                        }
                        res_per_pid.push(symbolized_record);
                    }

                    for (tid, samples) in &handler.raw_samples {
                         let thread_name = self.thread_names.get(tid).cloned();
                        let mut symbolized_record = SymbolizedRecord {
                            tid: *tid,
                            thread_name,
                            call_stacks: Vec::new(),
                        };

                        for sample in samples {
                            if let Ok(regs_data) = unwinder.set_sample_context(&sample.sample_memory) {
                                let frames = unwinder.unwind(regs_data, 128);
                                let mut current_call_stack = vec![];
                                for frame in frames {
                                    let bt = BacktraceDetails(frame.pc);
                                    let resolved_addr = seen_bt.entry(bt).or_insert_with_key(|bt_key| {
                                        let resolved_locations = symbolizer
                                            .resolve_addr(bt_key.0)
                                            .unwrap_or_default();
                                        ResolvedAddress {
                                            addr: bt_key.0,
                                            locations: resolved_locations,
                                        }
                                    }).to_owned();
                                    current_call_stack.push(resolved_addr);
                                }
                                symbolized_record.add_backtraces(current_call_stack);
                            }
                        }
                        if !symbolized_record.call_stacks.is_empty() {
                            res_per_pid.push(symbolized_record);
                        }
                    }

                    symbolizer.reset();
                    Ok((*pid, handler.process_name.clone(), res_per_pid))
        })
        .collect::<Result<Vec<(Pid, Option<String>, Vec<SymbolizedRecord>)>, SymbolizeError>>();
        symbolized_samples_per_thread
        }).collect::<Result<Vec<Vec<(Pid, Option<String>, Vec<SymbolizedRecord>)>>, SymbolizeError>>()?;
        let symbolized_samples = symbolized_samples.into_iter().flatten().collect();
        if !pprof_conversion {
            std::fs::write(output, format!("{symbolized_samples:#?}\n"))?;
        }
        Ok(SymbolizedRecords { records: symbolized_samples })
    }
}
