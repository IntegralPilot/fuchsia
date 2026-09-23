// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use fdomain_fuchsia_audio_device as fadevice;
use fdomain_fuchsia_io as fio;
use ffx_audio_device::device_list_untagged;
use ffx_audio_device::list::{ListResult, get_devices};
use ffx_audio_listdevices_args::ListDevicesCommand;
use ffx_writer::MachineWriter;
use fho::{FfxMain, FfxTool};
use fuchsia_audio_fdomain::Registry;
use target_holders::{exposed_dir, optional_moniker};

#[derive(FfxTool)]
pub struct ListDevicesTool {
    #[command]
    _cmd: ListDevicesCommand,
    #[with(exposed_dir("/bootstrap/devfs", "dev-class"))]
    dev_class: fio::DirectoryProxy,
    #[with(optional_moniker("/core/audio_device_registry"))]
    registry: Option<fadevice::RegistryProxy>,
}

fho::embedded_plugin!(ListDevicesTool);
#[async_trait(?Send)]
impl FfxMain for ListDevicesTool {
    type Writer = MachineWriter<ListResult>;

    type Error = ::fho::Error;

    async fn main(self, writer: Self::Writer) -> fho::Result<()> {
        let registry = self.registry.map(Registry::new);
        let selectors = get_devices(&self.dev_class, registry.as_ref()).await?;
        device_list_untagged(selectors, writer)
    }
}
