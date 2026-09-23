// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use fidl_fuchsia_process as fproc;
use fuchsia_runtime as runtime;
use std::sync::LazyLock;
use test_runners_lib::elf::{Component, KernelError};
use test_runners_lib::errors::*;
use test_runners_lib::launch;
use test_runners_lib::logs::LoggerStream;

static NEXT_VDSO: LazyLock<zx::NullableHandle> = LazyLock::new(|| {
    runtime::take_startup_handle(runtime::HandleInfo::new(runtime::HandleType::VdsoVmo, 0))
        .expect("failed to take next vDSO handle")
});

#[async_trait]
pub trait ComponentLauncher: Sized + Sync + Send {
    /// Convenience wrapper around [`launch::launch_process`].
    async fn launch_process(
        &self,
        component: &Component,
        args: Vec<String>,
    ) -> Result<(zx::Process, launch::ScopedJob, LoggerStream, LoggerStream), RunTestError>;
}

#[derive(Default)]
pub struct ElfComponentLauncher {}

#[async_trait]
impl ComponentLauncher for ElfComponentLauncher {
    /// Convenience wrapper around [`launch::launch_process`].
    async fn launch_process(
        &self,
        component: &Component,
        args: Vec<String>,
    ) -> Result<(zx::Process, launch::ScopedJob, LoggerStream, LoggerStream), RunTestError> {
        let (client_end, loader) = fidl::endpoints::create_endpoints();
        component.loader_service(loader);
        let executable_vmo = Some(component.executable_vmo()?);
        let handle_infos = vec![fproc::HandleInfo {
            handle: (*NEXT_VDSO)
                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                .map_err(launch::LaunchError::DuplicateVdso)?,
            id: runtime::HandleInfo::new(runtime::HandleType::VdsoVmo, 0).as_raw(),
        }];

        Ok(launch::launch_process_with_separate_std_handles(launch::LaunchProcessArgs {
            bin_path: &component.binary,
            process_name: &component.name,
            job: Some(component.job.create_child_job().map_err(KernelError::CreateJob).unwrap()),
            ns: component.ns.clone(),
            args: Some(args),
            name_infos: None,
            environs: component.environ.clone(),
            handle_infos: Some(handle_infos),
            loader_proxy_chan: Some(client_end.into_channel()),
            executable_vmo,
            options: component.options,
            config_vmo: component.config_vmo()?,
            url: Some(component.url.clone()),
            component_instance: component
                .component_instance
                .as_ref()
                .map(|c| c.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap()),
        })
        .await?)
    }
}

impl ElfComponentLauncher {
    pub fn new() -> Self {
        Self {}
    }
}
