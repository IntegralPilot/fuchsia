// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ebpf_api::{AttachType, ProgramType, StructId};
use fidl_fuchsia_ebpf as febpf;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

// The structs below must match the structs in `ebpf_test_progs.c`.
// LINT.IfChange

/// Configuration for the test program. If either field is not zero then the
/// test program will try to match these fields against the corresponding
/// fields in UDP packets. In that case, if a packet doesn't match or it's not
/// a UDP packet the program returns 0 without updating the `TestResult`. If
/// both fields are zero or the packet matches then `TestResult` is updated
/// and the program returns 1.
#[derive(Clone, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct TestConfig {
    /// Source port to match. Any port matches if set to 0.
    pub src_port: u16,
    /// Destination port to match. Any port matches if set to 0.
    pub dst_port: u16,
}

/// Struct used to store results of the last invocation of the test program
/// to test. The program copies the corresponding fields from the packet to
/// this struct. Test can read it using `TestProgram::read_test_result()`.
#[derive(Clone, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct TestResult {
    pub cookie: u64,
    pub uid: u32,
    pub ifindex: u32,
    pub ether_type: u32,
    pub mark: u32,
    pub src_port: u16,
    pub dst_port: u16,
    pub ip_proto: u8,
    pub _padding: [u8; 3],
}

#[derive(Clone, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct TestProgramState {
    pub config: TestConfig,
    pub _padding: [u8; 4],
    pub result: TestResult,
}

// LINT.ThenChange(//src/connectivity/network/testing/ebpf_test_util/ebpf/ebpf_test_progs.c)

pub struct TestProgramDefinition {
    program: ebpf::VerifiedEbpfProgram,
    maps: Vec<ebpf_loader::MapDefinition>,
}

impl TestProgramDefinition {
    /// Loads the test program, verifies it for the specified `program_type`.
    pub fn load(program_type: ProgramType) -> Self {
        let prog =
            ebpf_loader::load_ebpf_program("/pkg/data/ebpf_test_progs.o", ".text", "skb_test_prog")
                .expect("Failed to load test prog");
        let maps_schema = prog.maps.iter().map(|m| m.schema).collect();
        let calling_context = program_type
            .create_calling_context(AttachType::Unspecified, maps_schema)
            .expect("Failed to create CallingContext");
        let program =
            ebpf::verify_program(prog.code, calling_context, &mut ebpf::NullVerifierLogger)
                .expect("Failed to verify loaded program");
        Self { program, maps: prog.maps }
    }

    /// Initializes all maps used by the program.
    pub fn instantiate(&self) -> TestProgram {
        let maps = self
            .maps
            .iter()
            .map(|def| ebpf_api::Map::new(def.schema, &def.name()).expect("Failed to create a map"))
            .collect();

        let (handle, server_handle) = zx::EventPair::create();
        let handle = febpf::ProgramHandle { handle };
        TestProgram { program: self.program.clone(), maps, handle, server_handle }
    }
}

#[derive(Debug)]
pub struct TestProgram {
    handle: febpf::ProgramHandle,
    server_handle: zx::EventPair,
    program: ebpf::VerifiedEbpfProgram,
    maps: Vec<ebpf_api::PinnedMap>,
}

impl TestProgram {
    pub fn maps(&self) -> &[ebpf_api::PinnedMap] {
        &self.maps
    }

    pub fn get_fidl_program(&self) -> febpf::VerifiedProgram {
        let code: Vec<u64> =
            <[u64]>::ref_from_bytes(self.program.code().as_bytes()).unwrap().to_owned();
        let struct_access_instructions = self
            .program
            .struct_access_instructions()
            .iter()
            .map(|s| febpf::StructAccess {
                pc: s.pc.try_into().unwrap(),
                struct_id: StructId::try_from(&s.memory_id).unwrap().into(),
                field_offset: s.field_offset.try_into().unwrap(),
                is_32_bit_ptr_load: s.is_32_bit_ptr_load,
            })
            .collect();
        febpf::VerifiedProgram {
            code: Some(code),
            struct_access_instructions: Some(struct_access_instructions),
            maps: Some(self.maps.iter().map(|m| m.share().expect("share map")).collect()),
            __source_breaking: Default::default(),
        }
    }

    pub fn get_program_handle(&self) -> febpf::ProgramHandle {
        febpf::ProgramHandle {
            handle: self
                .handle
                .handle
                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                .expect("duplicate handle"),
        }
    }

    pub fn get_program_id(&self) -> febpf::ProgramId {
        febpf::ProgramId { id: self.handle.handle.koid().expect("get koid").raw_koid() }
    }

    // Mark the program defunct and return the server handle.
    pub fn mark_defunct(self) -> zx::EventPair {
        let Self { handle, server_handle, .. } = self;
        handle
            .handle
            .signal(
                zx::Signals::empty(),
                zx::Signals::from_bits_truncate(febpf::PROGRAM_DEFUNCT_SIGNAL),
            )
            .expect("signal EventPair");
        server_handle
    }

    pub fn read_test_state(&self) -> TestProgramState {
        let state = self.maps[0].load(&[0; 4]).expect("retrieve test state");
        TestProgramState::ref_from_bytes(&state).expect("convert test state struct").clone()
    }

    pub fn read_test_result(&self) -> TestResult {
        self.read_test_state().result
    }

    pub fn write_test_config(&self, config: TestConfig) {
        let mut state = self.read_test_state();
        state.config = config;
        self.maps[0].update(&[0; 4], state.as_mut_bytes().into(), 0).expect("store test state");
    }
}
