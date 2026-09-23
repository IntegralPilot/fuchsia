// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use ffx_config::EnvironmentContext;
use itertools::Itertools;
use schemars::JsonSchema;
use serde::Serialize;
use stacktrack_snapshot_fdomain as stacktrack_snapshot;
use std::collections::{HashMap, HashSet};
use std::io::Write;

fn serialize_hex<S>(val: &u64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&format!("{:#x}", val))
}

/// Structured source location information.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ResolvedLocation {
    pub function_name: Option<String>,
    pub file_name: Option<String>,
    pub line_number: Option<u32>,
}

/// An executable memory region and associated ELF module metadata.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ExecutableRegion {
    pub name: String,
    pub build_id: String,
    #[serde(serialize_with = "serialize_hex")]
    #[schemars(with = "String")]
    pub address: u64,
    #[serde(serialize_with = "serialize_hex")]
    #[schemars(with = "String")]
    pub size: u64,
    #[serde(serialize_with = "serialize_hex")]
    #[schemars(with = "String")]
    pub vaddr: u64,
}

/// A single call frame in a resolved stack trace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct CallFrame {
    /// Zero-based frame depth (0 is the leaf frame).
    pub level: usize,

    /// The program address (instruction pointer) for this call frame.
    #[serde(serialize_with = "serialize_hex")]
    #[schemars(with = "String")]
    pub pc: u64,

    /// Symbolized source locations (inlined call chain from leaf to root), if resolved.
    pub locations: Option<Vec<ResolvedLocation>>,

    /// Size of this stack frame in bytes, if known.
    pub frame_size: Option<usize>,

    /// The frame pointer represented as a negative offset relative to the thread's stack top.
    pub fp: i64,
}

/// A group of threads sharing an identical stack trace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct StackTraceGroup {
    /// Koids of all the threads that produced this stack trace.
    pub thread_koids: Vec<u64>,

    /// Stack depth in bytes consumed by a single thread in this group.
    pub stack_size: usize,

    /// Cumulative stack size in bytes across all threads in this group.
    pub total_size: usize,

    /// Sequence of call frames from leaf to root.
    pub frames: Vec<CallFrame>,
}

/// A complete symbolized stacktrack snapshot ready for output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ResolvedSnapshot {
    pub executable_regions: Vec<ExecutableRegion>,
    pub stack_trace_groups: Vec<StackTraceGroup>,
}

impl ResolvedSnapshot {
    pub fn new(
        context: Option<&EnvironmentContext>,
        raw_snapshot: &stacktrack_snapshot::Snapshot,
    ) -> Result<ResolvedSnapshot> {
        let resolver = match context {
            Some(context) => Some(SymbolResolver::new(context, raw_snapshot)?),
            None => None, // do not symbolize (test only)
        };

        let sorted_regions =
            raw_snapshot.executable_regions.iter().sorted_by_key(|(addr, _)| *addr);
        let executable_regions = sorted_regions
            .map(|(address, region)| ExecutableRegion {
                name: region.name.clone(),
                build_id: region.build_id.iter().map(|b| format!("{:02x}", b)).collect(),
                address: *address,
                size: region.size,
                vaddr: region.vaddr,
            })
            .collect();

        let sorted_groups = aggregate_and_sort_stack_traces(raw_snapshot);
        let stack_trace_groups = sorted_groups
            .into_iter()
            .map(|(stack_trace, thread_koids)| {
                let stack_size = stack_trace.stack_size();
                let total_size = stack_size * thread_koids.len();
                let mut frames = Vec::with_capacity(stack_trace.frames.len());
                let mut prev_fp: Option<i64> = None;
                for (level, frame) in stack_trace.frames.iter().enumerate() {
                    let frame_size = prev_fp.map(|p| frame.fp.wrapping_sub(p) as usize);
                    let locations = if let Some(resolver) = &resolver {
                        resolver.resolve_locations(frame.pc)
                    } else {
                        None
                    };
                    frames.push(CallFrame {
                        level,
                        pc: frame.pc,
                        locations,
                        frame_size,
                        fp: frame.fp,
                    });
                    prev_fp = Some(frame.fp);
                }
                StackTraceGroup { thread_koids, stack_size, total_size, frames }
            })
            .collect();

        Ok(ResolvedSnapshot { executable_regions, stack_trace_groups })
    }

    // Prints this snapshot as Markdown text.
    pub fn print_markdown<W: Write>(&self, mut writer: W) -> std::io::Result<()> {
        // Print executable regions.
        writeln!(writer, "# Executable regions (in symbolizer markup format)")?;
        writeln!(writer, "```")?;
        writeln!(writer, "{{{{{{reset}}}}}}")?;
        for (id, region) in self.executable_regions.iter().enumerate() {
            writeln!(writer, "{{{{{{module:{}:{}:elf:{}}}}}}}", id, region.name, region.build_id)?;
            writeln!(
                writer,
                "{{{{{{mmap:{:#x}:{:#x}:load:{}:r-x:{:#x}}}}}}}",
                region.address, region.size, id, region.vaddr
            )?;
        }
        writeln!(writer, "```")?;

        // Print stack trace groups.
        writeln!(writer, "# Stack traces")?;
        for (i, group) in self.stack_trace_groups.iter().enumerate() {
            writeln!(
                writer,
                "## Thread group {} - per-thread size: {}, total size: {}",
                i, group.stack_size, group.total_size
            )?;

            writeln!(writer, "<details>")?;
            writeln!(writer, "<summary>Number of threads: {}</summary>", group.thread_koids.len())?;
            writeln!(writer, "Thread koids: {:?}", group.thread_koids)?;
            writeln!(writer, "</details>")?;

            for frame in &group.frames {
                writeln!(writer)?;

                let size_str = if let Some(size) = frame.frame_size {
                    format!("{}", size)
                } else {
                    "?".to_string()
                };
                writeln!(
                    writer,
                    "**Level {}** (frame size = {} bytes, fp={}):",
                    frame.level, size_str, frame.fp
                )?;

                writeln!(writer, "```")?;
                if let Some(locations) = &frame.locations {
                    for location in locations {
                        let mut loc_str = location
                            .function_name
                            .clone()
                            .unwrap_or_else(|| format!("{:#x}", frame.pc));
                        if let Some(file) = &location.file_name {
                            if let Some(line) = location.line_number {
                                loc_str.push_str(&format!(" ({}:{})", file, line));
                            } else {
                                loc_str.push_str(&format!(" ({})", file));
                            }
                        }
                        writeln!(writer, "{}", loc_str)?;
                    }
                } else {
                    writeln!(writer, "{{{{{{bt:{}:{:#x}:pc}}}}}}", frame.level, frame.pc)?;
                }
                writeln!(writer, "```")?;
            }
        }
        Ok(())
    }
}

/// A stack trace where frame pointers are normalized relative to the top of the stack.
///
/// By expressing frame pointers as negative offsets from the top of the stack, stack traces from
/// different threads can be grouped together regardless of their absolute addresses in memory.
#[derive(PartialEq, Eq, Hash, Clone, Debug)]
struct NormalizedStackTrace {
    frames: Vec<NormalizedCallFrame>,
}

#[derive(PartialEq, Eq, Hash, Clone, Debug)]
struct NormalizedCallFrame {
    pc: u64,
    fp: i64,
}

impl NormalizedStackTrace {
    fn new(frames: &[stacktrack_snapshot::CallFrame], page_size: u64) -> NormalizedStackTrace {
        let mut normalized_frames = Vec::with_capacity(frames.len());

        if let Some(last_frame) = frames.last() {
            let stack_top = (last_frame.frame_pointer + page_size - 1) & !(page_size - 1);
            for frame in frames {
                normalized_frames.push(NormalizedCallFrame {
                    pc: frame.program_address,
                    fp: frame.frame_pointer.wrapping_sub(stack_top) as i64,
                });
            }
        }

        NormalizedStackTrace { frames: normalized_frames }
    }

    /// Returns the stack depth in bytes consumed by this stack trace.
    fn stack_size(&self) -> usize {
        self.frames.first().map_or(0, |frame| -frame.fp) as usize
    }
}

fn aggregate_and_sort_stack_traces(
    snapshot: &stacktrack_snapshot::Snapshot,
) -> Vec<(NormalizedStackTrace, Vec<u64>)> {
    let mut groups: HashMap<NormalizedStackTrace, Vec<u64>> = HashMap::new();
    for (thread_koid, stack_trace) in &snapshot.stack_traces {
        let key = NormalizedStackTrace::new(&stack_trace.frames, snapshot.page_size);
        groups.entry(key).or_default().push(*thread_koid);
    }

    // Sort by aggregated stack size, in descending order.
    groups
        .into_iter()
        .sorted_by_key(|(stack_trace, koids)| {
            std::cmp::Reverse(stack_trace.stack_size() * koids.len())
        })
        .collect_vec()
}

/// Helper wrapper around `Symbolizer` for resolving program addresses.
struct SymbolResolver {
    symbolizer: ffx_symbolize::Symbolizer,
}

impl SymbolResolver {
    fn new(
        context: &EnvironmentContext,
        snapshot: &stacktrack_snapshot::Snapshot,
    ) -> Result<SymbolResolver> {
        let mut sorted_regions: Vec<(u64, &stacktrack_snapshot::ExecutableRegion)> =
            snapshot.executable_regions.iter().map(|(k, v)| (*k, v)).collect();
        sorted_regions.sort_by_key(|(address, _)| *address);

        let mut referenced_regions = HashMap::with_capacity(sorted_regions.len());
        let mut searched_program_addresses = HashSet::new();
        for trace in snapshot.stack_traces.values() {
            for frame in &trace.frames {
                let program_address = frame.program_address;
                if searched_program_addresses.insert(program_address) {
                    let region_search_outcome =
                        sorted_regions.binary_search_by(|(region_starting_address, region)| {
                            if *region_starting_address + region.size <= program_address {
                                std::cmp::Ordering::Less
                            } else if program_address < *region_starting_address {
                                std::cmp::Ordering::Greater
                            } else {
                                referenced_regions.insert(*region_starting_address, region);
                                std::cmp::Ordering::Equal
                            }
                        });
                    if region_search_outcome.is_err() {
                        eprintln!(
                            "WARNING: No executable region found for program address {program_address:#016x}"
                        );
                    }
                }
            }
        }

        let mut symbolizer = ffx_symbolize::Symbolizer::with_context(context)?;
        let mut symbolizer_module_ids = HashMap::new();
        for (region_starting_address, region) in referenced_regions
            .into_iter()
            .sorted_by_key(|(region_starting_address, _)| *region_starting_address)
        {
            let base = region_starting_address.wrapping_sub(region.vaddr);
            let module_id = symbolizer_module_ids
                .entry((region.build_id.clone(), base))
                .or_insert_with(|| symbolizer.add_module(&region.name, &region.build_id));
            symbolizer.add_mapping(
                *module_id,
                ffx_symbolize::MappingDetails {
                    start_addr: region_starting_address,
                    size: region.size,
                    vaddr: region.vaddr,
                    flags: ffx_symbolize::MappingFlags::EXECUTE,
                },
            )?;
        }
        Ok(SymbolResolver { symbolizer })
    }

    /// Resolves an address to structured location info for all inlined frames
    /// (from innermost inlined function to the enclosing non-inlined function).
    fn resolve_locations(&self, address: u64) -> Option<Vec<ResolvedLocation>> {
        if let Ok(locations) = self.symbolizer.resolve_addr(address) {
            return Some(
                locations
                    .into_iter()
                    .map(|loc| {
                        let (file_name, line_number) = loc.file_and_line.unzip();
                        ResolvedLocation {
                            function_name: Some(loc.function),
                            file_name,
                            line_number,
                        }
                    })
                    .collect(),
            );
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value::Null;
    use stacktrack_snapshot_fdomain::{
        CallFrame as RawCallFrame, ExecutableRegion as RawExecutableRegion,
        Snapshot as RawSnapshot, StackTrace as RawStackTrace,
    };

    const MAP_1_ADDRESS: u64 = 0x100000;
    const MAP_1_SIZE: u64 = 0x1000;
    const MAP_1_VADDR: u64 = 0x0;
    const MAP_1_NAME: &str = "liba.so";
    const MAP_1_BUILD_ID: &str = "0123456789abcdef";

    const STACK_TRACE_A: [u64; 3] = [0x100010, 0x100020, 0x100030];
    const FP_0: u64 = 0x1B00;
    const FP_1: u64 = 0x1C00;
    const FP_2: u64 = 0x1E00;
    const THREAD_1_KOID: u64 = 1234;

    fn generate_fake_snapshot() -> RawSnapshot {
        RawSnapshot {
            page_size: 4096,
            stack_traces: [(
                THREAD_1_KOID,
                RawStackTrace {
                    frames: STACK_TRACE_A
                        .iter()
                        .zip([FP_0, FP_1, FP_2])
                        .map(|(addr, fp)| RawCallFrame {
                            program_address: *addr,
                            frame_pointer: fp,
                        })
                        .collect(),
                },
            )]
            .into_iter()
            .collect(),
            executable_regions: [(
                MAP_1_ADDRESS,
                RawExecutableRegion {
                    name: MAP_1_NAME.to_string(),
                    size: MAP_1_SIZE,
                    vaddr: MAP_1_VADDR,
                    build_id: hex::decode(MAP_1_BUILD_ID).unwrap(),
                },
            )]
            .into_iter()
            .collect(),
        }
    }

    #[test]
    fn test_resolved_snapshot() {
        let raw_snapshot = generate_fake_snapshot();
        let snapshot = ResolvedSnapshot::new(None, &raw_snapshot).unwrap();
        assert_eq!(snapshot.executable_regions.len(), 1);
        assert_eq!(snapshot.executable_regions[0].name, "liba.so");
        assert_eq!(snapshot.executable_regions[0].build_id, "0123456789abcdef");
        assert_eq!(snapshot.executable_regions[0].address, 0x100000);
        assert_eq!(snapshot.executable_regions[0].size, 0x1000);
        assert_eq!(snapshot.executable_regions[0].vaddr, 0x0);
        assert_eq!(snapshot.stack_trace_groups.len(), 1);
        assert_eq!(snapshot.stack_trace_groups[0].thread_koids, vec![1234]);
        assert_eq!(snapshot.stack_trace_groups[0].stack_size, 1280);
        assert_eq!(snapshot.stack_trace_groups[0].total_size, 1280);
        assert_eq!(snapshot.stack_trace_groups[0].frames.len(), 3);
        assert_eq!(snapshot.stack_trace_groups[0].frames[0].level, 0);
        assert_eq!(snapshot.stack_trace_groups[0].frames[0].frame_size, None);
        assert_eq!(snapshot.stack_trace_groups[0].frames[0].fp, -0x500);
        assert_eq!(snapshot.stack_trace_groups[0].frames[0].pc, 0x100010);
        assert_eq!(snapshot.stack_trace_groups[0].frames[0].locations, None);
        assert_eq!(snapshot.stack_trace_groups[0].frames[1].level, 1);
        assert_eq!(snapshot.stack_trace_groups[0].frames[1].frame_size, Some(256));
        assert_eq!(snapshot.stack_trace_groups[0].frames[1].fp, -0x400);
        assert_eq!(snapshot.stack_trace_groups[0].frames[1].pc, 0x100020);
        assert_eq!(snapshot.stack_trace_groups[0].frames[1].locations, None);
        assert_eq!(snapshot.stack_trace_groups[0].frames[2].level, 2);
        assert_eq!(snapshot.stack_trace_groups[0].frames[2].frame_size, Some(512));
        assert_eq!(snapshot.stack_trace_groups[0].frames[2].fp, -0x200);
        assert_eq!(snapshot.stack_trace_groups[0].frames[2].pc, 0x100030);
        assert_eq!(snapshot.stack_trace_groups[0].frames[2].locations, None);

        let json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(json["executable_regions"].as_array().unwrap().len(), 1);
        assert_eq!(json["executable_regions"][0]["name"], "liba.so");
        assert_eq!(json["executable_regions"][0]["build_id"], "0123456789abcdef");
        assert_eq!(json["executable_regions"][0]["address"], "0x100000");
        assert_eq!(json["executable_regions"][0]["size"], "0x1000");
        assert_eq!(json["executable_regions"][0]["vaddr"], "0x0");
        assert_eq!(json["stack_trace_groups"].as_array().unwrap().len(), 1);
        assert_eq!(
            json["stack_trace_groups"][0]["thread_koids"].as_array().unwrap(),
            &vec![serde_json::Value::from(1234)]
        );
        assert_eq!(json["stack_trace_groups"][0]["stack_size"], 1280);
        assert_eq!(json["stack_trace_groups"][0]["total_size"], 1280);
        assert_eq!(json["stack_trace_groups"][0]["frames"].as_array().unwrap().len(), 3);
        assert_eq!(json["stack_trace_groups"][0]["frames"][0]["level"], 0);
        assert_eq!(json["stack_trace_groups"][0]["frames"][0]["frame_size"], Null);
        assert_eq!(json["stack_trace_groups"][0]["frames"][0]["fp"], -0x500);
        assert_eq!(json["stack_trace_groups"][0]["frames"][0]["pc"], "0x100010");
        assert_eq!(json["stack_trace_groups"][0]["frames"][0]["locations"], Null);
        assert_eq!(json["stack_trace_groups"][0]["frames"][1]["level"], 1);
        assert_eq!(json["stack_trace_groups"][0]["frames"][1]["frame_size"], 256);
        assert_eq!(json["stack_trace_groups"][0]["frames"][1]["fp"], -0x400);
        assert_eq!(json["stack_trace_groups"][0]["frames"][1]["pc"], "0x100020");
        assert_eq!(json["stack_trace_groups"][0]["frames"][1]["locations"], Null);
        assert_eq!(json["stack_trace_groups"][0]["frames"][2]["level"], 2);
        assert_eq!(json["stack_trace_groups"][0]["frames"][2]["frame_size"], 512);
        assert_eq!(json["stack_trace_groups"][0]["frames"][2]["fp"], -0x200);
        assert_eq!(json["stack_trace_groups"][0]["frames"][2]["pc"], "0x100030");
        assert_eq!(json["stack_trace_groups"][0]["frames"][2]["locations"], Null);

        let mut output = Vec::new();
        snapshot.print_markdown(&mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("{{{module:0:liba.so:elf:0123456789abcdef}}}"));
        assert!(output_str.contains("{{{mmap:0x100000:0x1000:load:0:r-x:0x0}}}"));
        assert!(output_str.contains("Thread koids: [1234]"));
        assert!(output_str.contains("per-thread size: 1280, total size: 1280"));
        assert!(output_str.contains("{{{bt:0:0x100010:pc}}}"));
    }

    #[test]
    fn test_print_with_inlined_locations() {
        let raw_snapshot = generate_fake_snapshot();
        let mut snapshot = ResolvedSnapshot::new(None, &raw_snapshot).unwrap();

        // Simulate multi-location inlined call chain for frame 0
        snapshot.stack_trace_groups[0].frames[0].locations = Some(vec![
            ResolvedLocation {
                function_name: Some("inlined_leaf()".to_string()),
                file_name: Some("foo.rs".to_string()),
                line_number: Some(10),
            },
            ResolvedLocation {
                function_name: Some("outer_caller()".to_string()),
                file_name: Some("bar.rs".to_string()),
                line_number: Some(42),
            },
        ]);

        let mut output = Vec::new();
        snapshot.print_markdown(&mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("inlined_leaf() (foo.rs:10)\nouter_caller() (bar.rs:42)"));
    }
}
