// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! FFX plugin for examining product bundles, which are distributable containers for a product's
//! images and packages, and can be used to emulate, flash, or update a product.

use anyhow::{Context, Result, bail};

use ffx_writer::{SimpleWriter, ToolIO as _};
use fho::{FfxMain, FfxTool};
use product_bundle::{ProductBundle, ProductBundleV2};
use sdk_metadata::{VirtualDevice, VirtualDeviceManifest};
use serde_json::to_string_pretty;
use std::io::{stdin, stdout};
use structured_ui::{Notice, Presentation, TableRows};

mod args;
pub use args::ShowCommand;

/// `ffx product show` sub-command.
#[derive(FfxTool)]
pub struct ProductShowTool {
    #[command]
    pub cmd: ShowCommand,
}

/// Create a product bundle.
#[async_trait::async_trait(?Send)]
impl FfxMain for ProductShowTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        let mut input = stdin();
        let mut output = stdout();
        let mut err_out = writer.stderr();
        let ui = structured_ui::TextUi::new(&mut input, &mut output, &mut err_out);
        pb_show_impl(&ui, &self.cmd).await.map_err(Into::into)
    }
}

async fn pb_show_impl<I>(ui: &I, cmd: &ShowCommand) -> Result<()>
where
    I: structured_ui::Interface,
{
    log::debug!("pb_show");
    if !cmd.product_bundle_path.exists() {
        let mut note = Notice::builder();
        note.title(format!("File does not exist: {}", cmd.product_bundle_path));
        ui.present(&Presentation::Notice(note)).expect("Problem presenting the note.");
        return Ok(());
    }
    let product_bundle = ProductBundle::try_load_from(&cmd.product_bundle_path)?;

    if cmd.devices {
        if let Err(e) = list_virtual_devices(&product_bundle, ui).await {
            let mut note = Notice::builder();
            note.title(format!("{:?}", e));
            ui.present(&Presentation::Notice(note)).expect("Problem presenting the note.");
            return Ok(());
        }
    }
    if let Some(device_name) = &cmd.device {
        if let Err(e) = virtual_device_details(&product_bundle, ui, device_name).await {
            let mut note = Notice::builder();
            note.title(format!("{:?}", e));
            ui.present(&Presentation::Notice(note)).expect("Problem presenting the note.");
            return Ok(());
        }
    }
    Ok(())
}

async fn retrieve_v2_virtual_devices(
    product_bundle: &ProductBundleV2,
) -> Result<Vec<VirtualDevice>> {
    let path = product_bundle.get_virtual_devices_path();
    let manifest = VirtualDeviceManifest::from_path(&path).context("manifest from_path")?;
    let device_names = manifest.device_names();
    let mut devices = Vec::new();
    for name in device_names {
        devices.push(manifest.device(&name)?);
    }
    Ok(devices)
}

/// Given a product bundle, print the names and descriptions of all of the
/// virtual device specifications linked to that product bundle.
pub async fn list_virtual_devices<I>(product_bundle: &ProductBundle, ui: &I) -> Result<()>
where
    I: structured_ui::Interface,
{
    let virtual_devices = match product_bundle {
        ProductBundle::V2(product_bundle) => retrieve_v2_virtual_devices(product_bundle).await?,
    };
    let mut table = TableRows::builder();
    for entry in virtual_devices {
        match entry {
            VirtualDevice::V1(v) => {
                table.row(vec![
                    v.name,
                    v.description.unwrap_or_else(|| "No description.".to_string()),
                ]);
            }
        }
    }
    ui.present(&Presentation::Table(table.clone()))?;
    Ok(())
}

/// Given a device name, print the json-formatted contents of the virtual
/// device which matches that name. If no such device exists, return an error.
pub async fn virtual_device_details<I>(
    product_bundle: &ProductBundle,
    ui: &I,
    device_name: &str,
) -> Result<()>
where
    I: structured_ui::Interface,
{
    let virtual_devices = match product_bundle {
        ProductBundle::V2(product_bundle) => retrieve_v2_virtual_devices(product_bundle).await?,
    };
    let selected = virtual_devices.iter().find(|v| v.name() == device_name);
    if selected.is_none() {
        bail!("Couldn't find a virtual device named {}", device_name);
    }
    let mut notice = Notice::builder();
    let text = to_string_pretty(&selected)?;
    notice.message(text);
    ui.present(&Presentation::Notice(notice))?;
    Ok(())
}
