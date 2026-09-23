// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use clap::Parser;
use criterion::Criterion;
use ebpf::{EbpfBufferPtr, EbpfProgramContext, FieldMapping, ProgramArgument};
use ebpf_api::{
    __sk_buff, AttachType, CGROUP_SKB_SK_BUF_TYPE, LoadBytesBase, Map, MapValueRef,
    PacketWithLoadBytes, PinnedMap, ProgramType, SocketFilterProgramContext, SocketRef, uid_t,
};
use fuchsia_criterion::FuchsiaCriterion;
use std::time::Duration;

/// Benchmark configuration passed from the manifest file through the args.
#[derive(Parser, Debug)]
struct BenchmarkConfig {
    #[arg(long, required = true)]
    file: String,
    #[arg(long)]
    section: Vec<String>,
    #[arg(long)]
    name: Vec<String>,
}

struct TestEbpfContext {}

impl EbpfProgramContext for TestEbpfContext {
    type RunContext<'a> = TestEbpfRunContext<'a>;
    type Packet<'a> = ();
    type Map = PinnedMap;

    type Arg1<'a> = &'a SkBuff<'a>;
    type Arg2<'a> = ();
    type Arg3<'a> = ();
    type Arg4<'a> = ();
    type Arg5<'a> = ();
}

ebpf_api::ebpf_program_context_type!(TestEbpfContext, SocketFilterProgramContext);

#[repr(C)]
struct SkBuff<'a> {
    sk_buff: __sk_buff,

    data_ptr: *const u8,
    data_end_ptr: *const u8,

    uid: u32,
    socket_cookie: u64,
    data: &'a [u8],
}

impl ProgramArgument for &'_ SkBuff<'_> {
    fn get_type() -> &'static ebpf::Type {
        &*CGROUP_SKB_SK_BUF_TYPE
    }

    fn field_mappings() -> &'static [FieldMapping] {
        static FIELD_MAPPINGS: [FieldMapping; 2] = [
            FieldMapping {
                source_offset: std::mem::offset_of!(__sk_buff, data),
                target_offset: std::mem::offset_of!(SkBuff<'_>, data_ptr),
            },
            FieldMapping {
                source_offset: std::mem::offset_of!(__sk_buff, data_end),
                target_offset: std::mem::offset_of!(SkBuff<'_>, data_end_ptr),
            },
        ];
        &FIELD_MAPPINGS
    }
}

impl<'a> SocketRef for &'a SkBuff<'a> {
    fn get_socket_cookie(&self) -> Option<u64> {
        None
    }
    fn get_socket_uid(&self) -> Option<uid_t> {
        None
    }
}

#[derive(Default)]
struct TestEbpfRunContext<'a> {
    map_refs: Vec<MapValueRef<'a>>,
}

impl<'a> ebpf_api::MapsContext<'a> for TestEbpfRunContext<'a> {
    fn on_map_access(&mut self, _map: &ebpf_api::Map) {}

    fn add_value_ref(&mut self, map_ref: MapValueRef<'a>) {
        self.map_refs.push(map_ref)
    }
}

impl<'a> PacketWithLoadBytes for &'a SkBuff<'a> {
    fn load_bytes_relative(
        &self,
        base: LoadBytesBase,
        offset: usize,
        buf: EbpfBufferPtr<'_>,
    ) -> i64 {
        if base != LoadBytesBase::NetworkHeader {
            return -1;
        }

        let Some(data) = self.data.get(offset..(offset + buf.len())) else {
            return -1;
        };

        buf.store(data);
        0
    }
}

fn exit_on_error(msg: String) -> ! {
    eprintln!("{}", msg);
    std::process::exit(1);
}

// Fake IPv6 TCP packet.
const TEST_PACKET: [u8; 60] = [
    0x60, 0x00, 0x00, 0x00, 0x00, 0x1a, 0x06, 0x00, 0x20, 0x01, 0x0d, 0xb8, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x20, 0x01, 0x0d, 0xb8, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x04, 0xd2, 0x16, 0x2e, 0x00, 0x00, 0x00, 0x21,
    0x00, 0x00, 0x00, 0x00, 0x50, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
];

fn main() {
    // List of benchmark programs is passed as the argument list from the
    // component manifest. The arguments passed by the test executor are
    // separated from the arguments in the manifest file by adding "--" at
    // the end of the argument list in the manifest file.
    let mut args: Vec<_> = std::env::args().collect();
    let Some(separator_pos) = args.iter().position(|s| s == "--") else {
        eprintln!("{:?}", args);
        exit_on_error("-- not found in the argument list".to_string());
    };

    // Replace separate with the program name.
    args[separator_pos] = args[0].clone();

    let benchmark_args: Vec<_> = args[separator_pos..].iter().map(|s| &**s).collect();
    let mut fc = FuchsiaCriterion::fuchsia_bench_with_args(&benchmark_args[..]);
    let c: &mut Criterion = &mut fc;
    *c = std::mem::take(c)
        .warm_up_time(Duration::from_millis(1))
        .measurement_time(Duration::from_millis(100))
        .sample_size(100);

    // Parse benchmark config passed in the manifest file.
    let config = BenchmarkConfig::try_parse_from(args[..separator_pos].iter())
        .unwrap_or_else(|e| exit_on_error(format!("Failed to parse args: {}", e)));

    if config.section.len() != config.name.len() {
        eprintln!("The same number of --section and --name arguments is expected");
        std::process::exit(1);
    }

    let elf_file = ebpf_loader::ElfFile::new(&config.file).unwrap_or_else(|e| {
        exit_on_error(format!("Failed to load ELF file {}: {}", config.file, e))
    });
    for (section, name) in config.section.iter().zip(config.name.iter()) {
        let prog = ebpf_loader::load_ebpf_program_from_file(&elf_file, section, name)
            .unwrap_or_else(|e| exit_on_error(format!("Failed to load program {}: {}", name, e)));
        let maps_schema = prog.maps.iter().map(|m| m.schema).collect();
        let calling_context = ProgramType::CgroupSkb
            .create_calling_context(AttachType::CgroupInetEgress, maps_schema)
            .expect("Failed to create CallingContext");
        let verified =
            ebpf::verify_program(prog.code, calling_context, &mut ebpf::NullVerifierLogger)
                .unwrap_or_else(|e| exit_on_error(format!("Failed to verify {}: {}", name, e)));

        let maps: Vec<_> = prog
            .maps
            .iter()
            .map(|def| {
                Map::new(def.schema, &def.name())
                    .unwrap_or_else(|e| exit_on_error(format!("Failed to create a map: {:?}", e)))
            })
            .collect();

        let prog = ebpf::link_program::<TestEbpfContext>(&verified, maps)
            .unwrap_or_else(|e| exit_on_error(format!("Failed to link program {}: {}", name, e)));

        let mut group = c.benchmark_group("fuchsia.ebpf");
        let _ = group.bench_function(name, move |b| {
            let skb = SkBuff {
                sk_buff: __sk_buff {
                    len: TEST_PACKET.len() as u32,
                    protocol: 0xDD86, // hton(ETH_P_IPV6)
                    ifindex: 2,
                    ..__sk_buff::default()
                },

                data_ptr: TEST_PACKET.as_ptr(),

                // SAFETY: adding array size to the array base pointer.
                data_end_ptr: unsafe { TEST_PACKET.as_ptr().add(TEST_PACKET.len()) },

                uid: 52342,
                socket_cookie: 0x21ab3,
                data: &TEST_PACKET,
            };

            b.iter(|| {
                let mut ctx = TestEbpfRunContext::default();
                let _r = prog.run_with_1_argument(&mut ctx, &skb);
            })
        });
        group.finish();
    }
}
