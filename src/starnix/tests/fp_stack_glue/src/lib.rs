// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Test to ensure that we can use frame pointers to unwind a starnix Fuchsia stack into Linux
//! userspace.

use fidl_fuchsia_buildinfo as fbuildinfo;
use fidl_fuchsia_component_runner as frunner;
use fidl_fuchsia_sys2 as fsys;
use fuchsia_component::client::{connect_to_protocol, connect_to_protocol_at_dir_root};
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{
    Capability, ChildOptions, RealmBuilder, RealmBuilderParams, Ref, Route,
};
use futures::StreamExt;
use log::{info, warn};
use zx::Task;

#[fuchsia::test]
async fn frame_pointers_connect_from_fuchsia_to_linux() {
    let builder = RealmBuilder::with_params(
        RealmBuilderParams::new()
            .realm_name("fp_stack_glue_test")
            .from_relative_url("#meta/realm.cm"),
    )
    .await
    .unwrap();

    let (send_build_info_requests, mut recv_build_info_requests) =
        futures::channel::mpsc::unbounded();
    let build_info_mock = builder
        .add_local_child(
            "fake_build_info",
            move |handles| {
                let send_build_info_requests = send_build_info_requests.clone();
                Box::pin(async move {
                    let mut fs = ServiceFs::new();
                    fs.serve_connection(handles.outgoing_dir).unwrap();
                    fs.dir("svc").add_fidl_service(
                        |requests: fbuildinfo::ProviderRequestStream| {
                            send_build_info_requests.unbounded_send(requests).unwrap()
                        },
                    );
                    fs.collect::<()>().await;
                    Ok(())
                })
            },
            ChildOptions::new(),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fbuildinfo::ProviderMarker>())
                .from(&build_info_mock)
                .to(Ref::child("kernel")),
        )
        .await
        .unwrap();

    info!("starting realm");
    let realm = builder.build().await.unwrap();

    info!("waiting for linux program to park in the uname syscall");
    let mut build_info_requests = recv_build_info_requests.next().await.unwrap();
    let fbuildinfo::ProviderRequest::GetBuildInfo { responder } =
        build_info_requests.next().await.unwrap().unwrap();

    info!("finding uname-blocked thread handle using job handle in runtime dir");
    let realm_query = connect_to_protocol::<fsys::RealmQueryMarker>().unwrap();
    let (runtime_dir, runtime_dir_server) = fidl::endpoints::create_proxy();
    realm_query
        .open_directory(
            "realm_builder:fp_stack_glue_test/container",
            fsys::OpenDirType::RuntimeDir,
            runtime_dir_server,
        )
        .await
        .unwrap()
        .unwrap();
    let job_provider =
        connect_to_protocol_at_dir_root::<frunner::TaskProviderMarker>(&runtime_dir).unwrap();
    let container_job =
        job_provider.get_job().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();

    info!("have job handle for starnix container, finding uname-blocked process and thread");
    let mut print_uname_process = None;
    for process_koid in container_job.processes().unwrap() {
        let process = zx::Process::from(
            container_job.get_child(&process_koid, zx::Rights::SAME_RIGHTS).unwrap(),
        );
        if process.get_name().unwrap() == "print_uname_bin" {
            print_uname_process = Some(process);
            break;
        }
    }
    let print_uname_process = print_uname_process.unwrap();
    let print_uname_threads = print_uname_process.threads().unwrap();
    assert_eq!(print_uname_threads.len(), 1, "there should only be one thread in the test program");
    let print_uname_main_thread =
        print_uname_process.get_child(&print_uname_threads[0], zx::Rights::SAME_RIGHTS).unwrap();
    assert_eq!(
        print_uname_main_thread.info().unwrap().state,
        zx::ThreadState::Blocked(zx::ThreadBlockType::Channel),
        "thread should be blocked in a channel call for buildinfo"
    );

    info!("suspending thread and waiting for suspension to complete since it is not synchronous");
    let suspend_token = print_uname_main_thread.suspend().unwrap();
    while print_uname_main_thread.info().unwrap().state != zx::ThreadState::Suspended {}
    let register_state = print_uname_main_thread.read_state_general_regs().unwrap();
    drop(suspend_token);

    info!("have uname-blocked register state, unwinding stack with frame pointers");
    #[cfg(target_arch = "x86_64")]
    let leaf_frame_pointer = register_state.rbp;
    #[cfg(target_arch = "aarch64")]
    let leaf_frame_pointer = register_state.r[29];
    #[cfg(target_arch = "riscv64")]
    let leaf_frame_pointer = register_state.s0;
    let address_space_size = fuchsia_runtime::vmar_root_self().info().unwrap().len as u64;
    let mut found_starnix_pc = false;
    let mut found_linux_pc = false;
    let mut fp = leaf_frame_pointer;
    loop {
        let mut pc = 0u64.to_le_bytes();
        let pc_addr = fp as usize + 8;
        if let Err(e) = print_uname_process.read_memory(pc_addr, &mut pc) {
            warn!("Failed to read program counter at 0x{pc_addr:x}: {e:?}");
            break;
        }
        let pc = u64::from_le_bytes(pc);
        println!("pc: 0x{pc:x}");

        if pc >= address_space_size >> 1 {
            found_starnix_pc = true;
        } else {
            found_linux_pc = true;
        }

        let mut next_fp = 0u64.to_le_bytes();
        if let Err(e) = print_uname_process.read_memory(fp as usize, &mut next_fp) {
            warn!("Failed to read frame pointer 0x{fp:x}: {e:?}");
            break;
        }
        fp = u64::from_le_bytes(next_fp);
    }
    assert!(found_starnix_pc, "must have found program counters in starnix's half of aspace");
    assert!(found_linux_pc, "must have found program counters in userspace");

    info!("returning from buildinfo to unblock shutdown and destroying realm");
    responder.send(&fbuildinfo::BuildInfo::default()).unwrap();
    realm.destroy().await.unwrap();
}
