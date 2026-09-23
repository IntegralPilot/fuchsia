// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod address_space;
mod barriers;
mod context;
mod device;
mod device_task;
mod firmware;
mod hardware;
mod interfaces;
mod mem;
mod regs;
mod utils;

use crate::device::{DeviceClient, DeviceInterrupts, DeviceState};
use crate::device_task::CompletionEvent;
use crate::utils::LogError;
use fdf_component::{Driver, DriverContext, DriverError, Node, driver_register};
use mmio::ReadableRegister;
use pdev::{PdevExt as _, PlatformDevice};

struct MsdArmMaliCsf {
    device_client: DeviceClient,
    // We must keep this around to keep the driver alive.
    _node: Node,
}

driver_register!(MsdArmMaliCsf);

impl Driver for MsdArmMaliCsf {
    const NAME: &str = "msd_arm_mali_csf";

    async fn start(mut context: DriverContext) -> Result<Self, DriverError> {
        log::info!("Starting driver");
        let node = context.take_node()?;
        let pdev = context.connect_to_pdev()?;

        let mmio = pdev.map_mmio_by_id(0).await?;
        log::info!("GPU id: 0x{:x}", regs::GpuId::read(&mmio).0);
        log::info!("GPU coherency: {:?}", regs::CoherencyFeatures::read(&mmio));

        let bti =
            pdev.get_bti_by_id(0).await?.map_err(|s| s.err().unwrap_or(zx::Status::INTERNAL))?.bti;
        let smc: zx::NullableHandle = pdev
            .get_smc_by_id(0)
            .await?
            .map_err(|s| s.err().unwrap_or(zx::Status::INTERNAL))?
            .smc
            .into_handle()
            .into();
        let mapper = mem::CrosVmMapper::new(bti, smc);
        let interrupts = DeviceInterrupts::new(&pdev).await?;

        let state = DeviceState::new(mmio, Box::new(mapper));
        let device_client = DeviceClient::new(state, interrupts);

        // Go through the reset flow by requesting a reset and
        // waiting for the interrupt to fire.
        let reset_complete = CompletionEvent::new();
        let reset_complete_clone = reset_complete.clone();
        device_client.device_task_sender.send(Box::new(move |state| {
            fuchsia_async::Scope::current().spawn_local(async move {
                hardware::clear_interrupts(&mut state.borrow_mut().mmio);
                hardware::request_device_reset(&mut state.borrow_mut().mmio);
                let reset_signaller = state.borrow().reset_signaller.clone();
                reset_signaller.async_wait().await;
                hardware::power_on_l2(&mut state.borrow_mut().mmio).unwrap();
                hardware::enable_interrupts(&mut state.borrow_mut().mmio);
                reset_complete_clone.signal();
            });
        }));
        reset_complete.wait();

        // Load and parse the firmware.
        let firmware_vmo = crate::utils::load_file_to_vmo(&context, "/pkg/data/firmware.bin")
            .await
            .log_err("Failed to load firmware")?;
        let firmware_map = crate::mem::MappedMemory::new(
            &firmware_vmo,
            0,
            firmware_vmo.get_size()? as usize,
            zx::VmarFlags::PERM_READ,
        )?;
        let firmware = crate::firmware::parse_firmware(firmware_map.as_u8())
            .log_err("Failed to parse firmware")?;

        device_client.device_task_sender.send(Box::new(move |state| {
            state.borrow_mut().load_firmware(firmware).unwrap();
        }));

        log::info!("Finished starting driver");
        Ok(Self { device_client, _node: node })
    }

    async fn stop(&self) {
        log::info!("Stopping driver");
        self.device_client.device_task_sender.send(Box::new(move |state| {
            state.borrow_mut().should_shutdown = true;
        }));
        let Some(handle) = self.device_client.device_thread.lock().unwrap().take() else {
            return;
        };
        let _ = handle.join();
        log::info!("Finished stopping driver");
    }
}
