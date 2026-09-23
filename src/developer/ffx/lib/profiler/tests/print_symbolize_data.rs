// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use symbolize_test_utils::Mapping;
use symbolize_test_utils::collector::collect_modules;

const RECORD_TYPE_PROFILER: u64 = 10;
const SUBTYPE_MODULE: u64 = 0;
const SUBTYPE_MMAP: u64 = 1;
const SUBTYPE_BACKTRACE: u64 = 2;
const THREAD_REF_INLINE: u64 = 0;
const FXT_MAGIC_NUMBER: u64 = 0x0016547846040010;

fn pad8(len: usize) -> usize {
    (8 - (len % 8)) % 8
}

fn create_module_record(
    module_id: u16,
    name: &str,
    build_id: &[u8],
    pid: u64,
    tid: u64,
) -> Vec<u8> {
    let name_bytes = name.as_bytes();
    let name_padded_len = name_bytes.len() + pad8(name_bytes.len());
    let build_id_padded_len = build_id.len() + pad8(build_id.len());

    let payload_words = 3 + (name_padded_len / 8) + (build_id_padded_len / 8);
    let size_words = 1 + payload_words as u64;

    let header = (RECORD_TYPE_PROFILER << 0)
        | (size_words << 4)
        | (SUBTYPE_MODULE << 16)
        | (THREAD_REF_INLINE << 20)
        | ((module_id as u64) << 28)
        | ((name_bytes.len() as u64) << 44)
        | ((build_id.len() as u64) << 52);

    let mut record = Vec::new();
    record.extend_from_slice(&header.to_le_bytes());
    record.extend_from_slice(&0u64.to_le_bytes()); // timestamp
    record.extend_from_slice(&pid.to_le_bytes());
    record.extend_from_slice(&tid.to_le_bytes());

    record.extend_from_slice(name_bytes);
    record.resize(record.len() + pad8(name_bytes.len()), 0);

    record.extend_from_slice(build_id);
    record.resize(record.len() + pad8(build_id.len()), 0);

    record
}

fn create_mmap_record(module_id: u16, mapping: &Mapping, pid: u64, tid: u64) -> Vec<u8> {
    let mut flags: u64 = 0;
    if mapping.readable {
        flags |= 1;
    }
    if mapping.writeable {
        flags |= 2;
    }
    if mapping.executable {
        flags |= 4;
    }

    let size_words = 1 + 6;
    let header = (RECORD_TYPE_PROFILER << 0)
        | (size_words << 4)
        | (SUBTYPE_MMAP << 16)
        | (THREAD_REF_INLINE << 20)
        | ((module_id as u64) << 28)
        | (flags << 44);

    let mut record = Vec::new();
    record.extend_from_slice(&header.to_le_bytes());
    record.extend_from_slice(&0u64.to_le_bytes()); // timestamp
    record.extend_from_slice(&pid.to_le_bytes());
    record.extend_from_slice(&tid.to_le_bytes());
    record.extend_from_slice(&mapping.start_addr.to_le_bytes());
    record.extend_from_slice(&mapping.size.to_le_bytes());
    record.extend_from_slice(&mapping.vaddr.to_le_bytes());

    record
}

fn create_backtrace_record(addrs: &[u64], pid: u64, tid: u64) -> Vec<u8> {
    let frame_count = addrs.len() as u64;
    let size_words = 1 + 3 + frame_count;
    let header = (RECORD_TYPE_PROFILER << 0)
        | (size_words << 4)
        | (SUBTYPE_BACKTRACE << 16)
        | (THREAD_REF_INLINE << 20)
        | (frame_count << 28);

    let mut record = Vec::new();
    record.extend_from_slice(&header.to_le_bytes());
    record.extend_from_slice(&0u64.to_le_bytes()); // timestamp
    record.extend_from_slice(&pid.to_le_bytes());
    record.extend_from_slice(&tid.to_le_bytes());
    for &addr in addrs {
        record.extend_from_slice(&addr.to_le_bytes());
    }

    record
}

fn main() {
    let fxt_data = generate_unsymbolized_profile_data();
    let hex_encoded = hex::encode(&fxt_data);
    println!("{}", serde_json::to_string_pretty(&hex_encoded).unwrap());
}

fn generate_unsymbolized_profile_data() -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(&FXT_MAGIC_NUMBER.to_le_bytes());

    let pid = 12345;
    let tid = 54321;

    let modules = collect_modules();
    for (module_id, module) in modules.into_iter().enumerate() {
        data.extend_from_slice(&create_module_record(
            module_id as u16,
            &module.name,
            &module.build_id,
            pid,
            tid,
        ));
        for mapping in &module.mappings {
            data.extend_from_slice(&create_mmap_record(module_id as u16, mapping, pid, tid));
        }
    }

    let addrs = get_function_addr();
    data.extend_from_slice(&create_backtrace_record(&addrs, pid, tid));

    data
}

macro_rules! define_to_be_symbolized_function {
    ($t:expr) => {
        ::paste::paste! {
            pub fn [<to_be_symbolized_ $t>]() {
                println!(stringify!($t));
            }
        }
    };
}

define_to_be_symbolized_function!(1);
define_to_be_symbolized_function!(2);
define_to_be_symbolized_function!(3);
define_to_be_symbolized_function!(4);
define_to_be_symbolized_function!(5);

fn get_function_addr() -> Vec<u64> {
    vec![
        to_be_symbolized_1 as *const () as u64 + 1,
        to_be_symbolized_2 as *const () as u64 + 1,
        to_be_symbolized_3 as *const () as u64 + 1,
        to_be_symbolized_4 as *const () as u64 + 1,
        to_be_symbolized_5 as *const () as u64 + 1,
        zx::sys::zx_channel_create as *const () as u64 + 1,
    ]
}
