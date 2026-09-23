// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, anyhow};
use bootfs::{get_bootfs_vmo, get_zbi_container};
use cmdline::{ProgramInfo, parse_cmdline};
use fidl_next::{ClientEnd, ServerEnd};
use fidl_next_fuchsia_boot as fuchsia_boot;
use fidl_next_fuchsia_io as fuchsia_io;
use fuchsia_async::OnSignals;
use fuchsia_runtime::{HandleInfo, HandleType};
use process_builder::StartupHandle;
use program::{SystemHandles, launch_program};
use std::fmt::Write as _;
use std::ptr;
use userboot::{Log, take_system_handles};
use zbi::{ZbiContainer, ZbiType};
use zerocopy::IntoBytes as _;
use zx::sys::{ZX_SYSTEM_POWERCTL_SHUTDOWN, zx_system_powerctl};
use zx::{Channel, Rights, Signals};

const BOOT_TEST_SUCCESS_STRING: &str = env!("BOOT_TEST_SUCCESS_STRING");

/// Test configuration options parsed from the command line.
#[derive(Default)]
struct Options {
    /// Information about the test program to launch.
    test: ProgramInfo,
    /// Information about the primary boot program to launch.
    boot: ProgramInfo,
    /// Whether the next program to execute is a test.
    next_is_test: bool,
}

fn parse_option(key: &str, value: &str, opts: &mut Options) -> bool {
    match key {
        "userboot.test.next" => {
            opts.test.next = value.to_string();
            true
        }
        "userboot.test.root" => {
            opts.test.root = value.strip_suffix('/').unwrap_or(value).to_string();
            true
        }
        "userboot.next" => {
            opts.boot.next = value.to_string();
            true
        }
        "userboot.root" => {
            opts.boot.root = value.strip_suffix('/').unwrap_or(value).to_string();
            true
        }
        "userboot.next-is-test" => {
            opts.next_is_test = true;
            true
        }
        _ => false,
    }
}

fn get_options(container: &ZbiContainer<&[u8]>, log: &mut Log) -> Result<Options, Error> {
    let mut options = Options::default();
    for item in container.iter() {
        if item.header.type_ == ZbiType::CmdLine as u32 {
            if let Ok(cmdline) = str::from_utf8(item.payload.as_bytes()) {
                writeln!(log, "CMDLINE {}", cmdline)?;
                parse_cmdline(cmdline, log, &mut options, parse_option)?;
            }
        }
    }

    if !options.test.root.is_empty() && options.test.root.starts_with('/') {
        anyhow::bail!("`userboot.test.root` (\"{}\") must not begin with a '/'", options.test.root);
    }
    if !options.boot.root.is_empty() && options.boot.root.starts_with('/') {
        anyhow::bail!("`userboot.root` (\"{}\") must not begin with a '/'", options.boot.root);
    }

    Ok(options)
}

fn create_endpoints<P>() -> (ClientEnd<P, Channel>, ServerEnd<P, Channel>) {
    let (client, server) = Channel::create();
    (ClientEnd::from_untyped(client), ServerEnd::from_untyped(server))
}

async fn wait_for_process(
    process: &zx::Process,
    program_name: &str,
    log: &mut Log,
) -> Result<(), Error> {
    writeln!(log, "Waiting for {program_name} to exit...")?;
    let _ = OnSignals::new(process, Signals::PROCESS_TERMINATED).await?;
    let return_code = process.info()?.return_code;
    writeln!(log, "*** Exit status {return_code} ***")?;
    if return_code == 0 {
        writeln!(log, "{BOOT_TEST_SUCCESS_STRING}")?;
    }
    Ok(())
}

async fn run(log: &mut Log) -> Result<(), Error> {
    let mut handles = SystemHandles::from_handles(take_system_handles()?)?;
    let power_resource = handles
        .power_resource
        .as_ref()
        .map(|p| p.duplicate_handle(Rights::SAME_RIGHTS))
        .transpose()?;
    let root_vmar = fuchsia_runtime::vmar_root_self();

    let container = get_zbi_container(&handles.zbi_vmo, &root_vmar)?;
    let options = get_options(&container, log)?;
    let bootfs_vmo = get_bootfs_vmo(&container, &root_vmar, false, log)?;

    let (userboot_client, userboot_server) = create_endpoints::<fuchsia_boot::Userboot>();
    let (userboot_client, userboot_task) = userboot_client.spawn_full();
    let (svc_stash_client, svc_stash_server) = create_endpoints::<fuchsia_boot::SvcStash>();
    let (svc_stash_client, _svc_stash_task) = svc_stash_client.spawn_full();
    let _ = userboot_client.post_stash_svc(svc_stash_server).await;

    if !options.test.next.is_empty() {
        let (program_name, target_path) = options.test.filename();

        let (svc_client, svc_server) = create_endpoints::<fuchsia_io::Directory>();
        let _ = svc_stash_client.store(svc_server).await;

        let mut test_handles = handles.duplicate()?;
        test_handles.startup_handles.push(StartupHandle {
            handle: svc_client.into_untyped().into_handle().into(),
            info: HandleInfo::new(HandleType::NamespaceDirectory, 0),
        });
        let bootfs_vmo_dup = bootfs_vmo.duplicate_handle(Rights::SAME_RIGHTS)?;
        let process = launch_program(
            program_name,
            &target_path,
            options.test.next.split('+'),
            bootfs_vmo_dup,
            test_handles,
            None,
            log,
        )
        .await
        .map_err(|e| anyhow!("launch_program {program_name} failed: {e:?}"))?;

        wait_for_process(&process, program_name, log).await?;
    }

    if !options.boot.next.is_empty() {
        let (program_name, target_path) = options.boot.filename();

        let (svc_client, svc_server) = create_endpoints::<fuchsia_io::Directory>();
        let _ = svc_stash_client.store(svc_server).await;

        handles.startup_handles.push(StartupHandle {
            handle: svc_client.into_untyped().into_handle().into(),
            info: HandleInfo::new(HandleType::NamespaceDirectory, 0),
        });
        handles.startup_handles.push(StartupHandle {
            handle: userboot_server.into_untyped().into_handle().into(),
            info: HandleInfo::new(HandleType::User0, 0),
        });

        let mut bootfs_entries = Vec::new();
        let process = launch_program(
            program_name,
            &target_path,
            options.boot.next.split('+'),
            bootfs_vmo,
            handles,
            Some(&mut bootfs_entries),
            log,
        )
        .await
        .map_err(|e| anyhow!("launch_program {program_name} failed: {e:?}"))?;

        if !bootfs_entries.is_empty() {
            let files = bootfs_entries
                .into_iter()
                .map(|(offset, contents)| fuchsia_boot::natural::BootfsFileVmo { offset, contents })
                .collect::<Vec<_>>();
            let _ = userboot_client.post_bootfs_files(files).await;
        }
        userboot_client.close();
        drop(userboot_client);
        drop(userboot_task);

        if options.next_is_test {
            wait_for_process(&process, program_name, log).await?;
        }
    }

    if options.boot.next.is_empty() || options.next_is_test {
        if let Some(power_resource) = power_resource {
            writeln!(log, "Process exited, executing poweroff")?;
            // SAFETY: `power_resource` holds a valid resource handle required for poweroff.
            unsafe {
                zx_system_powerctl(
                    power_resource.raw_handle(),
                    ZX_SYSTEM_POWERCTL_SHUTDOWN,
                    ptr::null(),
                );
            }
            writeln!(log, "Still here after poweroff")?;
        }
    }

    Ok(())
}

#[fuchsia_async::run_singlethreaded]
async fn main() {
    let mut log = Log::new();
    if let Err(e) = run(&mut log).await {
        let _ = writeln!(log, "userboot error: {e:?}");
    }
}
