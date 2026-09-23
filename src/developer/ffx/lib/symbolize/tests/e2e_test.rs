// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ffx_e2e_emu::IsolatedEmulator;
use ffx_symbolize::{MappingDetails, MappingFlags, ResolveError, Symbolizer};
use log::info;
use symbolize_test_utils::{Module, SymbolizationTestOutputs};

#[fuchsia::test(logging_minimum_severity = "TRACE")]
async fn symbolize_fn_ptr() {
    logging_rust_cpp_bridge::init();

    info!("starting emulator...");
    let emu = IsolatedEmulator::start("test-ffx-symbolize-lib").await.unwrap();
    info!("running print_fn_ptr component...");
    let outputs = run_print_fn_ptr(&emu).await;

    info!("creating symbolizer instance...");
    let mut symbolizer = Symbolizer::with_context(emu.env_context()).unwrap();
    symbolize_and_validate_result(&outputs, &mut symbolizer);
    symbolizer.reset();
    let invalid_location_one = symbolizer.resolve_addr(outputs.fn_one_addr);
    assert!(invalid_location_one.is_err());
    symbolizer.reset();
    symbolize_and_validate_result(&outputs, &mut symbolizer);
}

fn symbolize_and_validate_result(outputs: &SymbolizationTestOutputs, symbolizer: &mut Symbolizer) {
    info!("adding modules to symbolizer...");
    for Module { name, build_id, mappings } in &outputs.modules {
        let id = symbolizer.add_module(&name, &build_id);
        for mapping in mappings {
            let mut flags = MappingFlags::empty();
            if mapping.readable {
                flags |= MappingFlags::READ;
            }
            if mapping.writeable {
                flags |= MappingFlags::WRITE;
            }
            if mapping.executable {
                flags |= MappingFlags::EXECUTE;
            }
            symbolizer
                .add_mapping(
                    id,
                    MappingDetails {
                        start_addr: mapping.start_addr,
                        size: mapping.size,
                        vaddr: mapping.vaddr,
                        flags,
                    },
                )
                .unwrap();
        }
    }

    info!("resolving addresses...");
    let symbol_one = symbolizer.resolve_addr(outputs.fn_one_addr).unwrap();
    assert_eq!(symbol_one.len(), 1);
    let location_one = &symbol_one[0];
    assert_eq!(location_one.function, "print_fn_ptr_bin::to_be_symbolized_one()");
    assert_eq!(
        location_one.file_and_line.as_ref().unwrap().0,
        "../../src/developer/ffx/lib/symbolize/tests/print_fn_ptr.rs"
    );
    assert!(
        outputs.fn_one_source_line - 2 <= location_one.file_and_line.as_ref().unwrap().1
            || location_one.file_and_line.as_ref().unwrap().1 <= outputs.fn_one_source_line + 2
    );
    assert_eq!(location_one.library, None);

    let symbol_two = symbolizer.resolve_addr(outputs.fn_two_addr).unwrap();
    assert_eq!(symbol_two.len(), 1);
    let location_two = &symbol_two[0];
    assert_eq!(location_two.function, "print_fn_ptr_bin::to_be_symbolized_two()");
    assert_eq!(
        location_two.file_and_line.as_ref().unwrap().0,
        "../../src/developer/ffx/lib/symbolize/tests/print_fn_ptr.rs"
    );
    assert!(
        outputs.fn_two_source_line - 2 <= location_two.file_and_line.as_ref().unwrap().1
            || location_two.file_and_line.as_ref().unwrap().1 <= outputs.fn_two_source_line + 2
    );
    assert_eq!(location_two.library, None);

    let libc_symbol = symbolizer.resolve_addr(outputs.libc_addr).unwrap();
    assert_eq!(libc_symbol.len(), 1);
    let libc_location = &libc_symbol[0];
    assert_eq!(libc_location.function, "open(const char*, int)");
    assert_eq!(libc_location.file_and_line.as_ref().unwrap().0, "../../sdk/lib/fdio/unistd.cc");
    assert_eq!(libc_location.library.as_ref().unwrap(), "libfdio.so");

    let symbol_sys_inc = symbolizer.resolve_addr(outputs.fn_sys_inc_addr).unwrap();
    assert_eq!(symbol_sys_inc.len(), 1);
    let sys_inc_location = &symbol_sys_inc[0];
    assert!(sys_inc_location.function.ends_with("zx_channel_create"));
    assert_eq!(
        sys_inc_location.file_and_line.as_ref().unwrap().0,
        "fidling/gen/zircon/vdso/zx/zither/kernel/lib/syscalls/syscalls.inc"
    );
    assert_eq!(sys_inc_location.library.as_ref().unwrap(), "<vDSO>");

    assert_eq!(
        symbolizer.resolve_addr(outputs.heap_addr).unwrap_err(),
        ResolveError::NoOverlappingModule
    );

    assert_eq!(
        symbolizer.resolve_addr(outputs.no_symbol_addr).unwrap_err(),
        ResolveError::SymbolNotFound
    );
}

async fn run_print_fn_ptr(emu: &IsolatedEmulator) -> SymbolizationTestOutputs {
    let stdout = emu
        .ffx_output(&[
            // JSON output prevents the command from printing its status messages to stdout.
            "--machine",
            "json",
            "component",
            "run",
            "/core/ffx-laboratory:print-fn-ptr",
            "fuchsia-pkg://fuchsia.com/print_fn_ptr#meta/print_fn_ptr.cm",
            // Ensure we get the component's stdout to the ffx command.
            "--connect-stdio",
        ])
        .await
        .unwrap();
    serde_json::from_str(&stdout).unwrap()
}

#[fuchsia::test(logging_minimum_severity = "TRACE")]
async fn symbolize_unresolved() {
    logging_rust_cpp_bridge::init();

    const FAKE_BUILD_ID: &[u8] = &[0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xCD, 0xEF];
    const FAKE_START_ADDRESS: u64 = 0x1000000;
    const FAKE_SIZE: u64 = 0x123000;

    let env = ffx_config::test_init().unwrap();

    let mut symbolizer = Symbolizer::with_context(&env.context).unwrap();
    let module_id = symbolizer.add_module("foo.so", FAKE_BUILD_ID);
    symbolizer
        .add_mapping(
            module_id,
            MappingDetails {
                start_addr: FAKE_START_ADDRESS,
                size: FAKE_SIZE,
                vaddr: 0,
                flags: MappingFlags::READ | MappingFlags::EXECUTE,
            },
        )
        .unwrap();

    assert_eq!(
        symbolizer.resolve_addr(FAKE_START_ADDRESS + FAKE_SIZE / 2).unwrap_err(),
        ResolveError::SymbolFileUnavailable
    );
}
