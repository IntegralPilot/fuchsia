// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ffx_symbolize::ResolvedLocation;
pub use profile_rust_proto::perfetto::third_party::perftools::profiles as pproto;
use profile_rust_proto::perfetto::third_party::perftools::profiles::{
    Function, Label, Line, Location, ValueType,
};

use crate::symbolize::{ResolvedAddress, SymbolizedRecords};

use prost::Message;
use std::collections::HashMap;
use std::default::Default;
use std::path::PathBuf;

static PID_INTERN: i64 = 1;
static TID_INTERN: i64 = 2;

#[derive(Eq, Hash, PartialEq, Debug)]
struct FunctionEntry {
    name: i64,
    filename: i64,
    start_line: i64,
}

#[derive(Eq, Hash, PartialEq, Debug)]
struct LocationEntry {
    addr: u64,
    functions: Vec<u64>, // a list of function id.
}

/// Pprof interns most strings: it keeps a table of known strings and replaces
/// most occurrences of strings in its data by their index. This structure
/// implements that behavior.
#[derive(Default)]
struct StringTable {
    strings: HashMap<String, i64>,
    table: Vec<String>,
}

impl StringTable {
    pub fn new() -> StringTable {
        let mut st = StringTable { strings: HashMap::new(), table: vec![] };
        st.intern("");
        st.intern("pid");
        st.intern("tid");
        st
    }
    /// If a string is already known to the table, return its index. Otherwise,
    /// insert it and return its newly created index.
    pub fn intern(&mut self, string: &str) -> i64 {
        *self.strings.entry(string.to_string()).or_insert_with(|| {
            let index = self.table.len();
            self.table.push(string.to_string());
            index as i64
        })
    }
    fn finalize(self) -> Vec<String> {
        self.table
    }
}

#[derive(Default)]
pub struct ProfileBuilder {
    profile: pproto::Profile,
    st: StringTable,
    function_to_index: HashMap<FunctionEntry, u64>,
    location_to_index: HashMap<LocationEntry, u64>,
}

impl ProfileBuilder {
    pub fn new() -> ProfileBuilder {
        ProfileBuilder {
            profile: pproto::Profile::default(),
            st: StringTable::new(),
            function_to_index: HashMap::new(),
            location_to_index: HashMap::new(),
        }
    }

    pub fn build_profile(mut self, input: SymbolizedRecords) -> pproto::Profile {
        for (pid, process_name, record_per_thread) in input.records {
            let pid_label_str = match &process_name {
                Some(name) => format!("process: {} (pid: {})", name, pid.0),
                None => format!("pid: {}", pid.0),
            };
            let pid_label = Label {
                key: PID_INTERN,
                str: self.st.intern(pid.to_string().as_str()),
                num: pid.0 as i64,
                ..Default::default()
            };
            let pid_addr = ResolvedAddress {
                addr: 0xDEADBEEF,
                locations: vec![ResolvedLocation {
                    function: pid_label_str,
                    file_and_line: None,
                    library: None,
                    library_offset: 0,
                }],
            };
            for record in record_per_thread {
                let tid_label_str = match &record.thread_name {
                    Some(name) => format!("thread: {} (tid: {})", name, record.tid.0),
                    None => format!("tid: {}", record.tid.0),
                };
                for call_stack in record.call_stacks {
                    let mut sample = pproto::Sample::default();
                    let tid_label = Label {
                        key: TID_INTERN,
                        str: self.st.intern(record.tid.to_string().as_str()),
                        num: record.tid.0 as i64,
                        ..Default::default()
                    };
                    sample.label.push(pid_label.clone());
                    sample.label.push(tid_label);
                    let tid_addr = ResolvedAddress {
                        addr: 0xDEADBEEF,
                        locations: vec![ResolvedLocation {
                            function: tid_label_str.clone(),
                            file_and_line: None,
                            library: None,
                            library_offset: 0,
                        }],
                    };
                    for backtrace in call_stack {
                        sample.location_id.push(self.build_location(backtrace));
                    }
                    sample.location_id.push(self.build_location(tid_addr));
                    sample.location_id.push(self.build_location(pid_addr.clone()));

                    sample.value.push(1);
                    self.profile.sample.push(sample);
                }
            }
        }
        self.profile.sample_type =
            vec![ValueType { r#type: self.st.intern("samples"), unit: self.st.intern("count") }];
        self.profile.string_table = self.st.finalize();
        self.profile
    }

    // Add location to profile struct, and return the related location id.
    pub fn build_location(&mut self, address: ResolvedAddress) -> u64 {
        let mut location = Location::default();
        location.address = address.addr;
        let mut function_id_list = vec![];
        for function in address.locations {
            let (function_id, line) = self.build_function(function);
            let line = Line { function_id, line };
            function_id_list.push(function_id);
            location.line.push(line);
        }
        let location_entry = LocationEntry { addr: address.addr, functions: function_id_list };

        let location_id = self.location_to_index.entry(location_entry).or_insert_with(|| {
            let next_id = self.profile.location.len() as u64 + 1;
            location.id = next_id;
            self.profile.location.push(location);

            next_id
        });

        *location_id
    }

    // Add function to the profile struct, and return the related function id and line number.
    pub fn build_function(&mut self, address: ResolvedLocation) -> (u64, i64) {
        let mut function = Function::default();
        function.name = self.st.intern(&address.function);
        let mut line_number = 0;
        if let Some((file, line)) = address.file_and_line {
            function.filename = self.st.intern(&file);
            function.start_line = line as i64;
            line_number = function.start_line;
        }
        let function_entry = FunctionEntry {
            name: function.name,
            filename: function.filename,
            start_line: function.start_line,
        };

        let function_id = self.function_to_index.entry(function_entry).or_insert_with(|| {
            let next_id = self.profile.function.len() as u64 + 1;
            function.id = next_id;
            self.profile.function.push(function);

            next_id
        });

        (*function_id, line_number)
    }
}

pub fn samples_to_pprof(input: SymbolizedRecords, output: PathBuf) -> Result<(), std::io::Error> {
    let builder = ProfileBuilder::new();
    let profile = builder.build_profile(input);
    std::fs::write(output, profile.encode_to_vec())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{Pid, Tid};
    use crate::symbolize::SymbolizedRecord;
    use assert_matches::assert_matches;
    // Helper to create a ResolvedAddress for testing
    fn make_resolved_address(addr: u64, function: &str, file: &str, line: u64) -> ResolvedAddress {
        ResolvedAddress {
            addr,
            locations: vec![ResolvedLocation {
                function: function.to_string(),
                file_and_line: Some((file.to_string(), line as u32)),
                library: Some("libtest.so".to_string()),
                library_offset: 0x1234,
            }],
        }
    }

    #[test]
    fn test_build_location_multiple_inlined() {
        let mut builder = ProfileBuilder::new();
        let addr = ResolvedAddress {
            addr: 0xABCD,
            locations: vec![
                ResolvedLocation {
                    function: "inlined_func".to_string(),
                    file_and_line: Some(("header.h".to_string(), 25)),
                    library: Some("libtest.so".to_string()),
                    library_offset: 0x1234,
                },
                ResolvedLocation {
                    function: "caller_func".to_string(),
                    file_and_line: Some(("caller.c".to_string(), 100)),
                    library: Some("libtest.so".to_string()),
                    library_offset: 0x1234,
                },
            ],
        };

        let loc_id = builder.build_location(addr);
        assert_eq!(loc_id, 1);
        assert_eq!(builder.profile.location.len(), 1);
        // Two functions should have been created
        assert_eq!(builder.profile.function.len(), 2);

        let location = &builder.profile.location[0];
        assert_eq!(location.id, 1);
        assert_eq!(location.address, 0xABCD);
        // There should be two lines, one for each inlined function
        assert_eq!(location.line.len(), 2);

        let inlined_func = &builder.profile.function[0];
        assert_eq!(builder.st.table[inlined_func.name as usize], "inlined_func");
        let caller_func = &builder.profile.function[1];
        assert_eq!(builder.st.table[caller_func.name as usize], "caller_func");

        assert_matches!(
            &location.line[..],
            [Line { function_id: 1, line: 25 }, Line { function_id: 2, line: 100 },]
        );
    }

    #[test]
    fn test_build_profile() {
        let builder = ProfileBuilder::new();
        let pid_val = 123u64;
        let tid_val = 456u64;

        let pid = Pid(pid_val);
        let tid = Tid(tid_val);

        let call_stack_trace1 = vec![
            make_resolved_address(0x1000, "main", "main.rs", 10),
            make_resolved_address(0x2000, "helper", "helper.rs", 5),
        ];

        let call_stack_trace2 = vec![
            make_resolved_address(0x1000, "main", "main.rs", 10),
            make_resolved_address(0x3000, "another", "another.rs", 20),
        ];

        let symbolized_record_for_tid = SymbolizedRecord {
            tid,
            thread_name: None,
            call_stacks: vec![call_stack_trace1, call_stack_trace2],
        };

        let input =
            SymbolizedRecords { records: vec![(pid, None, vec![symbolized_record_for_tid])] };

        let profile = builder.build_profile(input);

        // Expected strings: "", "pid", "tid", "123", "pid: 123", "456", "tid: 456",
        // "main", "main.rs", "helper", "helper.rs", "another", "another.rs", "samples", "count",
        assert_eq!(profile.string_table.len(), 15);
        assert_eq!(profile.sample_type.len(), 1);
        assert_eq!(profile.sample_type[0].r#type, 13); // "samples"
        assert_eq!(profile.sample_type[0].unit, 14); // "count"

        assert_eq!(profile.sample.len(), 2);
        assert_eq!(profile.function.len(), 5); // main, helper, another, pid, tid
        assert_eq!(profile.location.len(), 5); // 0x1000, 0x2000, 0x3000, pid_loc, tid_loc

        // --- Check Sample 1 ---
        let sample1 = &profile.sample[0];
        assert_eq!(sample1.value, vec![1]);
        assert_eq!(sample1.label.len(), 2); // pid, tid
        assert_eq!(sample1.label[0].key, PID_INTERN);
        assert_eq!(sample1.label[0].num, 123);
        assert_eq!(profile.string_table[sample1.label[0].str as usize], "123");

        assert_eq!(sample1.label[1].key, TID_INTERN);
        assert_eq!(sample1.label[1].num, 456);
        assert_eq!(profile.string_table[sample1.label[1].str as usize], "456");

        // Location IDs:
        // main_addr (0x1000) -> loc_id 1
        // helper_addr (0x2000) -> loc_id 2
        // tid_addr (func "tid:456") -> loc_id 3
        // pid_addr (func "pid:123") -> loc_id 4
        // another_addr (0x3000) -> loc_id 5
        // Sample 1 stack: [main_addr, helper_addr], tid_addr, pid_addr
        // Expected location_id: [loc_id(main), loc_id(helper), loc_id(tid), loc_id(pid)]
        assert_eq!(sample1.location_id, vec![1, 2, 3, 4]);

        // --- Check Sample 2 ---
        let sample2 = &profile.sample[1];
        // Sample 2 stack: [main_addr, another_addr], tid_addr, pid_addr
        // Expected location_id: [loc_id(main), loc_id(another), loc_id(tid), loc_id(pid)]
        assert_eq!(sample2.location_id, vec![1, 5, 3, 4]);

        // --- Check Locations ---
        let loc1 = profile.location.iter().find(|l| l.address == 0x1000).unwrap();
        assert_eq!(loc1.id, 1);
        let loc2 = profile.location.iter().find(|l| l.address == 0x2000).unwrap();
        assert_eq!(loc2.id, 2);
        let loc3 = profile.location.iter().find(|l| l.address == 0x3000).unwrap();
        assert_eq!(loc3.id, 5);
        let tid_loc = profile.location.iter().find(|l| l.id == 3).unwrap();
        assert_eq!(tid_loc.address, 0xDEADBEEF); // Pseudo address
        assert_eq!(
            profile.string_table
                [profile.function[tid_loc.line[0].function_id as usize - 1].name as usize],
            "tid: 456"
        );

        let pid_loc = profile.location.iter().find(|l| l.id == 4).unwrap();
        assert_eq!(pid_loc.address, 0xDEADBEEF); // Pseudo address
        assert_eq!(
            profile.string_table
                [profile.function[pid_loc.line[0].function_id as usize - 1].name as usize],
            "pid: 123"
        );

        // --- Check Functions ---
        let main_fn = profile
            .function
            .iter()
            .find(|f| profile.string_table[f.name as usize] == "main")
            .unwrap();
        assert_eq!(main_fn.id, 1);
        assert_eq!(profile.string_table[main_fn.filename as usize], "main.rs");
        assert_eq!(main_fn.start_line, 10);

        let helper_fn = profile
            .function
            .iter()
            .find(|f| profile.string_table[f.name as usize] == "helper")
            .unwrap();
        assert_eq!(helper_fn.id, 2);

        let tid_fn = profile
            .function
            .iter()
            .find(|f| profile.string_table[f.name as usize] == "tid: 456")
            .unwrap();
        assert_eq!(tid_fn.id, 3);

        let pid_fn = profile
            .function
            .iter()
            .find(|f| profile.string_table[f.name as usize] == "pid: 123")
            .unwrap();
        assert_eq!(pid_fn.id, 4);
    }
}
