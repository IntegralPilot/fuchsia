// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assert_matches::assert_matches;
use attribution_testing::PrincipalIdentifier;
use diagnostics_reader::ArchiveReader;

use fidl::endpoints::DiscoverableProtocolMarker;
use fidl_fuchsia_component::CreateChildArgs;
use fidl_fuchsia_component_decl::{Child, CollectionRef, StartupMode};
use fidl_fuchsia_sys2::OpenError;
use fuchsia_component_test::{
    RealmBuilder, RealmBuilderParams, RealmInstance, ScopedInstanceFactory,
};
use fuchsia_runtime::{HandleInfo, HandleType};
use futures::StreamExt;
use futures::stream::BoxStream;
use moniker::Moniker;
use regex::Regex;
use zx::{MapDetails, MapInfo, MappingDetails};
use {
    fidl_fuchsia_component as fcomponent, fidl_fuchsia_component_decl as fdecl,
    fidl_fuchsia_io as fio, fidl_fuchsia_memory_attribution as fattribution,
    fidl_fuchsia_process as fprocess, fidl_fuchsia_starnix_container as fcontainer,
    fidl_fuchsia_sys2 as fsys2,
};

const PROGRAM_COLLECTION: &str = "debian_programs";

#[fuchsia::test]
async fn mmap_anonymous() {
    const PROGRAM_URL: &str = "mmap_anonymous_then_sleep_package#meta/mmap_anonymous_then_sleep.cm";
    const EXPECTED_LOG: &str = "mmap_anonymous_then_sleep did mmap";

    let AttributionTest { realm, realm_proxy, container_execution, mut attribution } =
        init_attribution_test().await;

    // Stream memory attribution data until the container shows up in the reporting.
    let mut tree: attribution_testing::Principal;
    loop {
        tree = attribution.next().await.unwrap();
        if tree.children.len() != 0 {
            break;
        }
    }

    // Starnix runner should report a single container, backed by a job.
    assert_eq!(tree.children.len(), 2);
    let debian_container = tree.child_by_name("debian_container").unwrap();
    assert_eq!(debian_container.children.len(), 0);
    assert_eq!(debian_container.resources.len(), 1);
    let _starnix_kernel = tree.child_by_name("starnix_kernel").unwrap();

    // Use the container to start the Starnix program.
    let factory = ScopedInstanceFactory::new(PROGRAM_COLLECTION).with_realm_proxy(realm_proxy);
    let program = factory.new_instance(PROGRAM_URL).await.unwrap();

    // Wait for magic log from the program.
    wait_for_log(&realm, EXPECTED_LOG).await;

    // Wait for the desired attribution information.
    // There should be three child principals under the container:
    // - The init task (PID 1).
    // - The system task (PID 2).
    // - The task for the test program we just launched (PID > 2), and its name should
    //   match regex "(.*): mmap_anonymous_then_sleep".
    let program_name = Regex::new(r"(.*): mmap_anonymous_then_sleep").unwrap();
    loop {
        tree = attribution.next().await.unwrap();
        let container = &tree.child_by_name("debian_container").unwrap();
        if container
            .children
            .iter()
            .any(|child| program_name.is_match(child.name.as_str()) && child.resources.len() > 0)
        {
            break;
        }
    }
    let mut container = tree.child_by_name("debian_container").unwrap().clone();
    assert_eq!(container.children.len(), 3);
    let init = container
        .children
        .remove(container.children.iter().position(|c| c.name == "1: init").unwrap());
    assert_eq!(init.children.len(), 0);
    assert_eq!(init.resources.len(), 1, "init task should have one restricted VMAR");
    let system_task = container
        .children
        .remove(container.children.iter().position(|c| c.name == "2: [system task]").unwrap());
    assert_eq!(system_task.children.len(), 0);
    assert_eq!(system_task.resources.len(), 0, "system task should have zero restricted VMARs");
    let test_program = container.children.pop().unwrap();
    assert!(program_name.is_match(test_program.name.as_str()));
    assert_eq!(test_program.children.len(), 0);
    assert_eq!(test_program.resources.len(), 1, "test program should have one restricted VMAR");

    // We should receive the KOID of the test program process and the (base, len) information
    // of the restricted VMAR.
    //
    // The starnix process VMAR hierarchy looks like this:
    //
    //     /A ________00200000-0000400000000000     64.0T:sz                    'proc(restricted):5004903'
    //     /R ________00200000-0000400000000000     64.0T:sz                    'root'
    //     R  ________00200000-0000400000000000     64.0T:sz                    'useralloc'
    //      M 000000833189a000-000000833189b000 r--    4K:sz   0B:res 5004931:vmo 'blob-2a4670c5'
    //     ...
    //
    // and we are looking for a VMO allocated by the test program that is within
    // the restricted VMAR, with the expected number of populated pages.
    let resource = test_program.resources[0];
    let process = get_process_handle_by_name(&realm, "mmap_anonymous_then_sleep").await;
    assert_matches!(
        resource,
        attribution_testing::Resource::Vmar { process: process_koid, base, len }
        if process_koid == process.koid().unwrap() &&
            find_mapping_in_range(&process, |map, info| {
                // Within the restricted aspace.
                if map.base >= base && map.base + map.size <= base + len {
                    if info.populated_bytes == (4200 * zx::system_get_page_size()) as usize {  // See mmap_anonymous_then_sleep.c for size.
                        return true;
                    }
                }
                false
            })
    );

    // If we terminate the program, the tree should eventually no longer contain the program.
    drop(program);
    loop {
        tree = attribution.next().await.unwrap();
        if let Some(debian_container) = tree.child_by_name("debian_container") {
            if debian_container.children.len() == 2 {
                break;
            }
        }
    }

    // Stop the container and verify that the starnix runner reports that the
    // container is removed.
    container_execution.stop().unwrap();
    let event = container_execution.take_event_stream().next().await.unwrap().unwrap();
    assert_matches!(event, fcomponent::ExecutionControllerEvent::OnStop { .. });
    loop {
        let tree_opt = attribution.next().await;
        if tree_opt.is_none() {
            break;
        }
    }
}

#[fuchsia::test]
async fn leader_killed() {
    const PROGRAM_URL: &str =
        "thread_group_leader_killed_package#meta/thread_group_leader_killed.cm";
    const EXPECTED_LOG: &str = "[thread_group_leader_killed] second thread did mmap";

    let AttributionTest { realm, realm_proxy, container_execution, mut attribution } =
        init_attribution_test().await;

    // Stream memory attribution data until the container shows up in the reporting.
    let mut tree: attribution_testing::Principal;
    loop {
        tree = attribution.next().await.unwrap();
        if tree.children.len() != 0 {
            break;
        }
    }
    // Starnix runner reports two principals: (i) the container, backed by a job, (ii) the
    // starnix_kernel.
    assert_eq!(tree.children.len(), 2);
    let debian_container = tree.child_by_name("debian_container").unwrap();
    assert_eq!(debian_container.children.len(), 0);
    assert_eq!(debian_container.resources.len(), 1);
    let _starnix_kernel = tree.child_by_name("starnix_kernel").unwrap();

    // Use the container to start the Starnix program.
    let factory = ScopedInstanceFactory::new(PROGRAM_COLLECTION).with_realm_proxy(realm_proxy);
    let program = factory.new_instance(PROGRAM_URL).await.unwrap();

    // Wait for magic log from the program.
    wait_for_log(&realm, EXPECTED_LOG).await;

    // Wait for the desired attribution information.
    // There should be three child principals under the container:
    // - The init task (PID 1).
    // - The system task (PID 2).
    // - The task for the test program we just launched (PID > 2), and its name should
    //   match regex "(.*): thread_group_leader_killed".
    let program_name = Regex::new(r"(.*): thread_group_leader_killed").unwrap();
    loop {
        tree = attribution.next().await.unwrap();
        let container = &tree.child_by_name("debian_container").unwrap();
        if container
            .children
            .iter()
            .any(|child| program_name.is_match(child.name.as_str()) && child.resources.len() > 0)
        {
            break;
        }
    }
    let mut container = tree.child_by_name("debian_container").unwrap().clone();
    assert_eq!(container.children.len(), 3);
    container.children.remove(container.children.iter().position(|c| c.name == "1: init").unwrap());
    container
        .children
        .remove(container.children.iter().position(|c| c.name == "2: [system task]").unwrap());

    let test_program = container.children.pop().unwrap();
    assert!(program_name.is_match(test_program.name.as_str()));
    assert_eq!(test_program.children.len(), 0);
    assert_eq!(test_program.resources.len(), 1, "test program should have one restricted VMAR");

    // Termination memory reporting is tested in `mmap_anonymous`.
    drop(program);
    drop(container_execution);
}

/// Test that a change in the name of a process results in an attribution update.
#[fuchsia::test]
async fn process_name_change() {
    const PROGRAM_URL: &str = "process_name_change_package#meta/process_name_change.cm";
    const EXPECTED_LOG_1: &str = "process_name_change started";
    const EXPECTED_LOG_2: &str = "process_name_change name changed";

    let AttributionTest { realm, realm_proxy, container_execution, mut attribution } =
        init_attribution_test().await;

    // Stream memory attribution data until the container shows up in the reporting.
    let mut tree: attribution_testing::Principal;
    loop {
        tree = attribution.next().await.unwrap();
        if tree.children.len() != 0 {
            break;
        }
    }

    // Starnix runner reports two principals: (i) the container, backed by a job, (ii) the
    // starnix_kernel.
    assert_eq!(tree.children.len(), 2);
    let debian_container = tree.child_by_name("debian_container").unwrap();
    assert_eq!(debian_container.children.len(), 0);
    assert_eq!(debian_container.resources.len(), 1);
    let _starnix_kernel = tree.child_by_name("starnix_kernel").unwrap();

    let (stdin_recv, stdin_send) = zx::Socket::create_stream();

    // We need to manually start the process to be able to communicate with it through its standard
    // input.
    realm_proxy
        .create_child(
            &CollectionRef { name: PROGRAM_COLLECTION.to_string() },
            &Child {
                name: Some("process_name_change".to_string()),
                url: Some(PROGRAM_URL.to_string()),
                startup: Some(StartupMode::Lazy),
                ..Default::default()
            },
            CreateChildArgs {
                numbered_handles: Some(vec![fprocess::HandleInfo {
                    id: HandleInfo::new(HandleType::FileDescriptor, 0).as_raw(),
                    handle: stdin_recv.into(),
                }]),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();

    // Wait for log from the program.
    wait_for_log(&realm, EXPECTED_LOG_1).await;

    stdin_send.write(b"n\n").unwrap();

    wait_for_log(&realm, EXPECTED_LOG_2).await;

    // Wait for the desired attribution information.
    // There should be three child principals under the container:
    // - The init task (PID 1).
    // - The system task (PID 2).
    // - The task for the test program we just launched (PID > 2), and its name should
    //   match regex "(.*): process_name_change".
    let program_name = Regex::new(r"(.*): new_name").unwrap();
    loop {
        tree = attribution.next().await.unwrap();
        let container = &tree.child_by_name("debian_container").unwrap();
        if container
            .children
            .iter()
            .any(|child| program_name.is_match(child.name.as_str()) && child.resources.len() > 0)
        {
            break;
        }
    }

    // Terminate the program. The tree should eventually no longer contain the program.
    stdin_send.write(b"n\n").unwrap();

    loop {
        tree = attribution.next().await.unwrap();
        if let Some(debian_container) = tree.child_by_name("debian_container") {
            if debian_container.children.len() == 2 {
                break;
            }
        }
    }

    // Stop the container and verify that the starnix runner reports that the
    // container is removed.
    container_execution.stop().unwrap();
    let event = container_execution.take_event_stream().next().await.unwrap().unwrap();
    assert_matches!(event, fcomponent::ExecutionControllerEvent::OnStop { .. });
    loop {
        let tree_opt = attribution.next().await;
        if tree_opt.is_none() {
            break;
        }
    }
}

struct AttributionTest {
    realm: RealmInstance,
    realm_proxy: fcomponent::RealmProxy,
    container_execution: fcomponent::ExecutionControllerProxy,
    attribution: BoxStream<'static, attribution_testing::Principal>,
}

async fn init_attribution_test() -> AttributionTest {
    let builder = RealmBuilder::with_params(
        RealmBuilderParams::new().from_relative_url("#meta/realm.cm").start(false),
    )
    .await
    .expect("created");
    let realm = builder.build().await.expect("build test realm");

    // Start the container and obtain its execution controller.
    let (container_controller, server_end) =
        fidl::endpoints::create_proxy::<fcomponent::ControllerMarker>();
    let realm_proxy: fcomponent::RealmProxy =
        realm.root.connect_to_protocol_at_exposed_dir().unwrap();
    realm_proxy
        .open_controller(
            &fdecl::ChildRef { name: "debian_container".to_string(), collection: None },
            server_end,
        )
        .await
        .unwrap()
        .expect("open_controller");
    let (container_execution, server_end) =
        fidl::endpoints::create_proxy::<fcomponent::ExecutionControllerMarker>();
    container_controller
        .start(fcomponent::StartChildArgs::default(), server_end)
        .await
        .unwrap()
        .expect("start debian container");

    let introspector = realm.root.connect_to_protocol_at_exposed_dir().unwrap();

    // Connect to the attribution protocol of the starnix kernel. We need to use a RealmQuery as the
    // exact name of the starnix kernel moniker is random, so unknown to the test at this point.
    let realm_query = realm.root.connect_to_protocol_at_exposed_dir().unwrap();
    let starnix_kernel_moniker = loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
        match find_starnix_kernel_moniker(&realm_query).await {
            Some(moniker) => break moniker,
            None => continue,
        };
    };

    let attribution_provider = loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
        match connect_to_service_in_exposed_dir::<fattribution::ProviderMarker>(
            &realm_query,
            &starnix_kernel_moniker,
        )
        .await
        {
            Ok(provider) => break provider,
            Err(OpenError::InstanceNotResolved) => continue,
            Err(e) => panic!("Error opening the starnix kernel memory provider: {:#?}", e),
        }
    };

    let attribution = attribution_testing::attribute_memory(
        PrincipalIdentifier(1),
        "starnix_runner".to_string(),
        attribution_provider,
        introspector,
    );
    AttributionTest { realm, realm_proxy, container_execution, attribution }
}

async fn wait_for_log(realm: &RealmInstance, expected_log: &str) {
    let mut reader = ArchiveReader::logs();
    let selector: Moniker = realm.root.moniker().parse().unwrap();
    let selector = format!("{}/**", selector);
    reader.select_all_for_component(selector.as_str());
    let mut logs = reader.snapshot_then_subscribe().unwrap();
    loop {
        let message = logs.next().await.unwrap().unwrap();
        if message.msg().unwrap().contains(expected_log) {
            break;
        }
    }
}

async fn get_process_handle_by_name(realm: &RealmInstance, name: &str) -> zx::Process {
    // Get the job of the starnix kernel.
    let realm_query = realm.root.connect_to_protocol_at_exposed_dir().unwrap();
    let starnix_kernel_moniker = find_starnix_kernel_moniker(&realm_query).await.unwrap();
    let starnix_controller = connect_to_service_in_exposed_dir::<fcontainer::ControllerMarker>(
        &realm_query,
        &starnix_kernel_moniker,
    )
    .await
    .unwrap();
    let starnix_kernel_job = starnix_controller.get_job_handle().await.unwrap().job.unwrap();

    // Find the process named with the given name in the job.
    let process_koids = starnix_kernel_job.processes().unwrap();
    for koid in process_koids {
        let process = starnix_kernel_job.get_child(&koid, zx::Rights::SAME_RIGHTS).unwrap();
        if &process.get_name().unwrap() == name {
            let process: zx::Process = process.into();
            return process;
        }
    }

    panic!("Did not find test starnix program");
}

async fn find_starnix_kernel_moniker(realm_query: &fsys2::RealmQueryProxy) -> Option<String> {
    // Enumerate the instances until we find the starnix kernel.
    let iterator = realm_query.get_all_instances().await.unwrap().unwrap();
    let iterator = iterator.into_proxy();
    while let Ok(instances) = iterator.next().await {
        for instance in instances {
            if let Some(url) = instance.url {
                if url.contains("starnix_kernel") {
                    return Some(instance.moniker.unwrap());
                }
            }
        }
    }

    None
}

/// Connects to a named service in the component's exposed directory referenced by `moniker`
/// relative to `realm`.
async fn connect_to_service_in_exposed_dir<T: DiscoverableProtocolMarker>(
    realm: &fsys2::RealmQueryProxy,
    moniker: &str,
) -> Result<T::Proxy, OpenError> {
    let (exposed_dir, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
    realm.open_directory(moniker, fsys2::OpenDirType::ExposedDir, server_end).await.unwrap()?;
    let (service, server_end) = fidl::endpoints::create_proxy::<T>();
    exposed_dir
        .open(
            T::PROTOCOL_NAME,
            fio::Flags::PROTOCOL_SERVICE,
            &Default::default(),
            server_end.into_channel(),
        )
        .unwrap();
    Ok(service)
}

/// Find a mapping in the process that satisfies the predicate.
fn find_mapping_in_range(
    process: &zx::Process,
    predicate: impl Fn(&MapInfo, &MappingDetails) -> bool,
) -> bool {
    let maps = process.info_maps_vec().unwrap();
    for map in maps {
        match map.details() {
            MapDetails::Mapping(info) => {
                if predicate(&map, &info) {
                    return true;
                }
            }
            _ => (),
        }
    }
    false
}
