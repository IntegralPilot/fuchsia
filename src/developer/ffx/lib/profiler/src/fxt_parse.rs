// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::parse::{
    BacktraceDetails, ModuleDetails, ModuleWithMmapDetails, Pid, RawSample, SymbolizeError, Tid,
    UnsymbolizedSamples,
};
use ffx_symbolize::{MappingDetails, MappingFlags};
use fxt::TraceRecord;
use fxt::profiler::ProfilerRecord;
use fxt::session::SessionParser;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

impl UnsymbolizedSamples {
    pub fn new_from_fxt_file(input: impl AsRef<Path>) -> Result<Self, SymbolizeError> {
        let file = File::open(input.as_ref())?;
        let reader = BufReader::new(file);
        let mut parser = SessionParser::new(reader);
        let mut unsymbolized = Self { handlers: HashMap::new(), thread_names: HashMap::new() };
        while let Some(record) = parser.next() {
            let record = record?;
            match record {
                TraceRecord::Profiler(profiler_record) => match profiler_record {
                    ProfilerRecord::Module(module) => {
                        let pid = Pid(module.process.0);
                        let handler = unsymbolized.handlers.entry(pid).or_default();
                        let module_details = ModuleDetails {
                            name: module.name.to_string(),
                            build_id: hex::encode(module.build_id),
                        };
                        handler.module_with_mmap_records.entry(module.module_id).or_insert_with(
                            || ModuleWithMmapDetails { module: module_details, mmaps: Vec::new() },
                        );
                    }
                    ProfilerRecord::Mapping(mapping) => {
                        let pid = Pid(mapping.process.0);
                        let handler = unsymbolized
                            .handlers
                            .get_mut(&pid)
                            .ok_or(SymbolizeError::InvalidMappingRecord)?;
                        let ModuleWithMmapDetails { mmaps, .. } = handler
                            .module_with_mmap_records
                            .get_mut(&mapping.module_id)
                            .ok_or(SymbolizeError::InvalidMappingRecord)?;
                        let flags = MappingFlags::from_bits_truncate(mapping.flags as u32);
                        mmaps.push(MappingDetails {
                            start_addr: mapping.start_addr,
                            size: mapping.range,
                            flags,
                            vaddr: mapping.vaddr,
                        });
                    }
                    ProfilerRecord::Backtrace(backtrace) => {
                        let pid = Pid(backtrace.process.0);
                        let tid = Tid(backtrace.thread.0);
                        let handler = unsymbolized.handlers.entry(pid).or_default();
                        let backtraces = handler.backtrace_records.entry(tid).or_default();
                        let details = backtrace.data.into_iter().map(BacktraceDetails).collect();
                        backtraces.push(details);
                    }
                },
                TraceRecord::KernelObj(kernel_obj) => {
                    if kernel_obj.ty == fxt::objects::KernelObjType::Process {
                        let pid = Pid(kernel_obj.koid);
                        let handler = unsymbolized.handlers.entry(pid).or_default();
                        handler.process_name = Some(kernel_obj.name.to_string());
                    } else if kernel_obj.ty == fxt::objects::KernelObjType::Thread {
                        let tid = Tid(kernel_obj.koid);
                        unsymbolized.thread_names.insert(tid, kernel_obj.name.to_string());
                    }
                }
                TraceRecord::LargeBlob(blob) => {
                    if blob.name == "stack_sample" && blob.category == "cpu_profiler" {
                        if let Some(metadata) = blob.metadata {
                            let pid = Pid(metadata.process.0);
                            let tid = Tid(metadata.thread.0);
                            let handler = unsymbolized.handlers.entry(pid).or_default();
                            let samples = handler.raw_samples.entry(tid).or_default();
                            let timestamp = metadata.timestamp as u64;
                            samples.push(RawSample { timestamp, sample_memory: blob.bytes });
                        }
                    } else {
                        return Err(SymbolizeError::NonProfilerFxtRecord);
                    }
                }
                _ => return Err(SymbolizeError::NonProfilerFxtRecord),
            }
        }

        Ok(unsymbolized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::ProfilingRecordHandler;
    use std::io::Write;
    use std::path::PathBuf;
    use tempfile::NamedTempFile;

    #[test]
    fn test_set_up_profiling_record_handlers_with_fxt_file() {
        let mut profiler_record = NamedTempFile::new().expect("Failed to create temp file");
        // --- FXT Record Construction Logic ---

        const RECORD_TYPE_PROFILER: u64 = 10;
        const SUBTYPE_MODULE: u64 = 0;
        const SUBTYPE_MMAP: u64 = 1;
        const SUBTYPE_BACKTRACE: u64 = 2;
        const THREAD_REF_INLINE: u64 = 0; // Denotes inline PID/TID

        // The 64-bit magic number for FXT files.
        const FXT_MAGIC_NUMBER: u64 = 0x0016547846040010;

        /// Creates a 64-bit FXT Module Record header.
        fn create_module_header(
            size_words: u64,
            module_id: u64,
            name_len: u64,
            build_id_len: u64,
        ) -> [u8; 8] {
            let header = (RECORD_TYPE_PROFILER << 0)
                | (size_words << 4)
                | (SUBTYPE_MODULE << 16)
                | (THREAD_REF_INLINE << 20)
                | (module_id << 28)
                | (name_len << 44)
                | (build_id_len << 52);
            header.to_le_bytes()
        }

        /// Creates a 64-bit FXT Mmap Record header.
        fn create_mmap_header(size_words: u64, module_id: u64, flags: u64) -> [u8; 8] {
            let header = (RECORD_TYPE_PROFILER << 0)
                | (size_words << 4)
                | (SUBTYPE_MMAP << 16)
                | (THREAD_REF_INLINE << 20)
                | (module_id << 28)
                | (flags << 44); // Flags: READ=1, EXECUTE=4. READ|EXECUTE=5
            header.to_le_bytes()
        }

        /// Creates a 64-bit FXT Backtrace Record header.
        fn create_backtrace_header(size_words: u64, frame_count: u64) -> [u8; 8] {
            let header = (RECORD_TYPE_PROFILER << 0)
                | (size_words << 4)
                | (SUBTYPE_BACKTRACE << 16)
                | (THREAD_REF_INLINE << 20)
                | (frame_count << 28);
            header.to_le_bytes()
        }

        let mut profiler_record_content = Vec::<u8>::new();

        profiler_record_content.extend_from_slice(&FXT_MAGIC_NUMBER.to_le_bytes());

        // --- Process 1 (PID 1104) ---

        // Record 1: Module Record for "libtrace-engine.so"
        profiler_record_content.extend_from_slice(&create_module_header(10, 0, 18, 20));
        profiler_record_content.extend_from_slice(&[
            // Timestamp
            0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // PID (1104)
            0x50, 0x4, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // TID (0)
            0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
            // Name: "libtrace-engine.so" (18 bytes + 6 padding = 24 bytes)
            0x6c, 0x69, 0x62, 0x74, 0x72, 0x61, 0x63, 0x65, 0x2d, 0x65, 0x6e, 0x67, 0x69, 0x6e,
            0x65, 0x2e, 0x73, 0x6f, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
            // Build ID: "333e...a8ae" (20 bytes + 4 padding = 24 bytes)
            0x33, 0x3e, 0x89, 0xf0, 0xc1, 0x75, 0x00, 0x0c, 0xee, 0x9b, 0x7e, 0x20, 0x1f, 0xed,
            0xcd, 0x6f, 0x9b, 0x4b, 0xa8, 0xae, 0x0, 0x0, 0x0, 0x0,
        ]);

        // Record 2: Mmap Record
        profiler_record_content.extend_from_slice(&create_mmap_header(
            7,
            0,
            MappingFlags::READ.bits().into(),
        ));
        profiler_record_content.extend_from_slice(&[
            0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // Timestamp
            0x50, 0x4, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // PID (1104)
            0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // TID (0)
            0x0, 0x60, 0x39, 0x36, 0xc9, 0x0, 0x0, 0x0, // Start Address: 0xc936396000
            0x0, 0x60, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // Size: 0x6000
            0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // Vaddr: 0x0
        ]);

        // Record 3: Mmap Record
        profiler_record_content.extend_from_slice(&create_mmap_header(
            7,
            0,
            (MappingFlags::READ | MappingFlags::EXECUTE).bits().into(),
        ));
        profiler_record_content.extend_from_slice(&[
            0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // Timestamp
            0x50, 0x4, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // PID (1104)
            0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // TID (0)
            0x0, 0xc0, 0x39, 0x36, 0xc9, 0x0, 0x0, 0x0, // Start Address: 0xc93639c000
            0x0, 0xd0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // Size: 0xd000
            0x0, 0x60, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // Vaddr: 0x6000
        ]);

        // Record 4: Backtrace Record
        profiler_record_content.extend_from_slice(&create_backtrace_header(6, 2));
        profiler_record_content.extend_from_slice(&[
            0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // Timestamp
            0x50, 0x4, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // PID (1104)
            0x38, 0xa, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // TID (2616)
            0x10, 0x8e, 0x7f, 0x38, 0xdc, 0x43, 0x0, 0x0, // Frame 1: 0x43dc387f8e10
            0x6c, 0xa1, 0xff, 0x69, 0xb0, 0x2, 0x0, 0x0, // Frame 2: 0x2b069ffa16c
        ]);

        // Record 5: Backtrace Record
        profiler_record_content.extend_from_slice(&create_backtrace_header(6, 2));
        profiler_record_content.extend_from_slice(&[
            0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // Timestamp
            0x50, 0x4, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // PID (1104)
            0xca, 0x4, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // TID (1226)
            0x10, 0x8e, 0x7f, 0x38, 0xdc, 0x43, 0x0, 0x0, // Frame 1: 0x43dc387f8e10
            0x5e, 0xc8, 0xc4, 0x56, 0xa6, 0x3, 0x0, 0x0, // Frame 2: 0x3a656c4c85e
        ]);

        // --- Process 2 (PID 4207) ---

        // Record 6: Module Record for "<VMO#4165=/boot/bin/sh>"
        profiler_record_content.extend_from_slice(&create_module_header(10, 0, 23, 20));
        profiler_record_content.extend_from_slice(&[
            0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // Timestamp
            0x6f, 0x10, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // PID (4207)
            0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // TID(0)
            // Name: "<VMO#4165=/boot/bin/sh>" (23 bytes + 1 padding = 24 bytes)
            0x3c, 0x56, 0x4d, 0x4f, 0x23, 0x34, 0x31, 0x36, 0x35, 0x3d, 0x2f, 0x62, 0x6f, 0x6f,
            0x74, 0x2f, 0x62, 0x69, 0x6e, 0x2f, 0x73, 0x68, 0x3e, 0x0,
            // Build ID: "867c...ba0" (20 bytes + 4 bytes padding)
            0x86, 0x7c, 0x18, 0x81, 0x85, 0x84, 0xf5, 0x82, 0x3f, 0x35, 0x47, 0x2b, 0x70, 0xfc,
            0x87, 0x14, 0xb2, 0x51, 0x8b, 0xa0, 0x0, 0x0, 0x0, 0x0,
        ]);

        // Record 7: Mmap Record
        profiler_record_content.extend_from_slice(&create_mmap_header(
            7,
            0,
            MappingFlags::READ.bits().into(),
        ));
        profiler_record_content.extend_from_slice(&[
            0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // Timestamp
            0x6f, 0x10, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // PID (4207)
            0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // TID (0)
            0x0, 0xd0, 0x23, 0xb5, 0xd, 0x3, 0x0, 0x0, // Start Address: 0x30db523d000
            0x0, 0x0, 0x1, 0x0, 0x0, 0x0, 0x0, 0x0, // Size: 0x10000
            0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // Vaddr: 0x0
        ]);

        // Record 8: Module Record for "libfdio.so"
        profiler_record_content.extend_from_slice(&create_module_header(9, 1, 10, 20));
        profiler_record_content.extend_from_slice(&[
            0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // Timestamp
            0x6f, 0x10, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // PID (4207)
            0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // TID (0)
            // Name: "libfdio.so" (10 bytes + 6 padding = 16 bytes)
            0x6c, 0x69, 0x62, 0x66, 0x64, 0x69, 0x6f, 0x2e, 0x73, 0x6f, 0x0, 0x0, 0x0, 0x0, 0x0,
            0x0, // Build ID: "3e1c...9867" (20 bytes + 4 padding = 24 bytes)
            0x3e, 0x1c, 0x4e, 0xb8, 0x2f, 0x79, 0xaf, 0x6a, 0x4f, 0xd1, 0x42, 0xdb, 0x11, 0xf8,
            0x39, 0x79, 0x77, 0x2f, 0x98, 0x67, 0x0, 0x0, 0x0, 0x0,
        ]);

        // Record 9: Mmap Record
        profiler_record_content.extend_from_slice(&create_mmap_header(
            7,
            1,
            MappingFlags::READ.bits().into(),
        ));
        profiler_record_content.extend_from_slice(&[
            0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // Timestamp
            0x6f, 0x10, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // PID (4207)
            0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // TID (0)
            0x0, 0x30, 0x2b, 0xd8, 0xbf, 0x3, 0x0, 0x0, // Start Address: 0x3bfd82b3000
            0x0, 0x20, 0x6, 0x0, 0x0, 0x0, 0x0, 0x0, // Size: 0x62000
            0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // Vaddr: 0x0
        ]);

        // Record 10: Backtrace Record
        profiler_record_content.extend_from_slice(&create_backtrace_header(6, 2));
        profiler_record_content.extend_from_slice(&[
            0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // Timestamp
            0x6f, 0x10, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // PID (4207)
            0x71, 0x10, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // TID (4209)
            0xea, 0xdc, 0xd1, 0xc, 0x1c, 0x40, 0x0, 0x0, // Frame 1: 0x401c0cd1dcea
            0x94, 0xdb, 0x34, 0xd8, 0xbf, 0x3, 0x0, 0x0, // Frame 2: 0x3bfd834db94
        ]);

        // Record 11: Backtrace Record
        profiler_record_content.extend_from_slice(&create_backtrace_header(7, 3));
        profiler_record_content.extend_from_slice(&[
            0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // Timestamp
            0x6f, 0x10, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // PID (4207)
            0x71, 0x10, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, // TID (4209)
            0xea, 0xdc, 0xd1, 0xc, 0x1c, 0x40, 0x0, 0x0, // Frame 1: 0x401c0cd1dcea
            0x94, 0xdb, 0x34, 0xd8, 0xbf, 0x3, 0x0, 0x0, // Frame 2: 0x3bfd834db94
            0xb, 0xe8, 0x34, 0xd8, 0xbf, 0x3, 0x0, 0x0, // Frame 3: 0x3bfd834e80b
        ]);

        // --- End of FXT Record Construction ---

        profiler_record.write_all(&profiler_record_content).expect("Failed to write to temp file");

        profiler_record.flush().expect("Failed to flush");
        let profiler_record_path: PathBuf = profiler_record.path().to_path_buf();
        let handlers = UnsymbolizedSamples::new_from_fxt_file(&profiler_record_path).unwrap();
        let first_handler = ProfilingRecordHandler {
            process_name: None,
            module_with_mmap_records: HashMap::from([(
                0,
                ModuleWithMmapDetails {
                    module: ModuleDetails {
                        name: "libtrace-engine.so".to_string(),
                        build_id: "333e89f0c175000cee9b7e201fedcd6f9b4ba8ae".to_string(),
                    },
                    mmaps: vec![
                        MappingDetails {
                            start_addr: 0xc936396000,
                            size: 0x6000,
                            vaddr: 0x0,
                            flags: MappingFlags::READ,
                        },
                        MappingDetails {
                            start_addr: 0xc93639c000,
                            size: 0xd000,
                            vaddr: 0x6000,
                            flags: MappingFlags::READ | MappingFlags::EXECUTE,
                        },
                    ],
                },
            )]),
            backtrace_records: HashMap::from([
                (
                    Tid(2616),
                    vec![vec![BacktraceDetails(0x43dc387f8e10), BacktraceDetails(0x2b069ffa16c)]],
                ),
                (
                    Tid(1226),
                    vec![vec![BacktraceDetails(0x43dc387f8e10), BacktraceDetails(0x3a656c4c85e)]],
                ),
            ]),
            ..Default::default()
        };

        let second_handler = ProfilingRecordHandler {
            process_name: None,
            module_with_mmap_records: HashMap::from([
                (
                    0,
                    ModuleWithMmapDetails {
                        module: ModuleDetails {
                            name: "<VMO#4165=/boot/bin/sh>".to_string(),
                            build_id: "867c18818584f5823f35472b70fc8714b2518ba0".to_string(),
                        },
                        mmaps: vec![MappingDetails {
                            start_addr: 0x30db523d000,
                            size: 0x10000,
                            vaddr: 0x0,
                            flags: MappingFlags::READ,
                        }],
                    },
                ),
                (
                    1,
                    ModuleWithMmapDetails {
                        module: ModuleDetails {
                            name: "libfdio.so".to_string(),
                            build_id: "3e1c4eb82f79af6a4fd142db11f83979772f9867".to_string(),
                        },
                        mmaps: vec![MappingDetails {
                            start_addr: 0x3bfd82b3000,
                            size: 0x62000,
                            vaddr: 0x0,
                            flags: MappingFlags::READ,
                        }],
                    },
                ),
            ]),
            backtrace_records: HashMap::from([(
                Tid(4209),
                vec![
                    vec![BacktraceDetails(0x401c0cd1dcea), BacktraceDetails(0x3bfd834db94)],
                    vec![
                        BacktraceDetails(0x401c0cd1dcea),
                        BacktraceDetails(0x3bfd834db94),
                        BacktraceDetails(0x3bfd834e80b),
                    ],
                ],
            )]),
            ..Default::default()
        };
        let mut expected_handlers = HashMap::new();
        expected_handlers.insert(Pid(4207), second_handler);
        expected_handlers.insert(Pid(1104), first_handler);
        assert_eq!(
            UnsymbolizedSamples { handlers: expected_handlers, thread_names: HashMap::new() },
            handlers
        );
    }
}
