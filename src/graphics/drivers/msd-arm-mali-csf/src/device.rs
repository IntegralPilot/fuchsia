// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::address_space::{AddressSpace, Coherency};
use crate::device_task::{CompletionEvent, DeviceTask, DeviceTaskReceiver, DeviceTaskSender};
use crate::mem::BusMapper;
use crate::utils::LogError;
use crate::{context, firmware, hardware, interfaces, mem, regs, utils};
use fdf_component::DriverError;
use fidl_next_fuchsia_hardware_platform_device as fpdev;
use futures::StreamExt;
use mmio::{ReadableRegister, WritableRegister};
use std::cell::RefCell;
use std::sync::Mutex;

use mmio::region::MmioRegion;
use mmio::vmo::VmoMemory;

use std::sync::Arc;

pub struct DeviceState {
    pub reset_signaller: CompletionEvent,
    pub mmio: MmioRegion<VmoMemory>,
    pub should_shutdown: bool,
    pub bus_mapper: Box<dyn BusMapper>,
    pub firmware: Vec<firmware::Section>,
    pub interface_memory: Option<mem::MappedMemory>,
    pub shared_interface: Option<interfaces::global::Interface>,
    pub address_spaces: Vec<AddressSpace>,
    pub groups: Vec<context::Group>,
}

impl DeviceState {
    pub fn new(mmio: MmioRegion<VmoMemory>, bus_mapper: Box<dyn BusMapper>) -> Self {
        DeviceState {
            mmio,
            reset_signaller: CompletionEvent::new(),
            bus_mapper,
            should_shutdown: false,
            firmware: Vec::new(),
            interface_memory: None,
            shared_interface: None,
            address_spaces: Vec::new(),
            groups: Vec::new(),
        }
    }

    async fn gpu_irq_task(state: Arc<RefCell<Self>>, gpu_irq: zx::Interrupt) {
        let mut gpu_irq = std::pin::pin!(fuchsia_async::OnInterrupt::new(gpu_irq));
        while let Some(_) = gpu_irq.next().await {
            state.borrow_mut().handle_gpu_irq((*gpu_irq).as_ref());
        }
    }

    fn handle_gpu_irq(&mut self, interrupt: &zx::Interrupt) {
        let irq_status = regs::GpuIrqMask::read_status(&self.mmio);
        if irq_status.reset_completed() != 0 {
            self.reset_signaller.signal();
            self.reset_signaller.reset();
        } else if irq_status.mcu_status() != 0 {
            log::warn!("MCU status: {:#?}", regs::McuStatus::read(&self.mmio));
        } else if irq_status.0 != 0 {
            log::warn!("Unknown gpu irq: {:#?}", irq_status);
        }

        irq_status.to_clear().write(&mut self.mmio);
        if let Err(error) = interrupt.ack() {
            log::error!("interrupt ack failed: {}", error);
        }
    }

    async fn mmu_irq_task(state: Arc<RefCell<Self>>, irq: zx::Interrupt) {
        let mut irq = std::pin::pin!(fuchsia_async::OnInterrupt::new(irq));
        while let Some(_) = irq.next().await {
            state.borrow_mut().handle_mmu_irq((*irq).as_ref());
        }
    }

    fn handle_mmu_irq(&mut self, interrupt: &zx::Interrupt) {
        let status = regs::MmuIrqRawStatus::read(&self.mmio);
        log::error!("mmu     status 0x{:x}", regs::MmuIrqStatus::read(&self.mmio).0);

        let address_space_index = (status.0 | (status.0 >> 16)).trailing_zeros() as u64;
        let address_space_regs = regs::AddressSpaceRegs::new(address_space_index);
        let fault_status = address_space_regs.read_fault_status(&self.mmio);
        let address = address_space_regs.read_fault_address(&self.mmio);
        log::error!(
            "Page fault in Address Space {}\n\
            Virtual Address: 0x{:x}\n\
            Fault Status: {:#?}",
            address_space_index,
            address,
            fault_status
        );

        regs::MmuIrqClear(status.0).write(&mut self.mmio);
        if let Err(error) = interrupt.ack() {
            log::error!("interrupt ack failed: {}", error);
        }
    }

    async fn job_irq_task(state: Arc<RefCell<Self>>, irq: zx::Interrupt) {
        let mut irq = std::pin::pin!(fuchsia_async::OnInterrupt::new(irq));
        while let Some(_) = irq.next().await {
            state.borrow_mut().handle_job_irq((*irq).as_ref());
        }
    }

    fn handle_job_irq(&mut self, interrupt: &zx::Interrupt) {
        let status = regs::JobIrqStatus::read(&self.mmio);
        regs::JobIrqClear(status.0).write(&mut self.mmio);

        if let Err(error) = interrupt.ack() {
            log::error!("interrupt ack failed: {}", error);
        }

        if status.global_interface_ready() == 1 && self.shared_interface.is_none() {
            log::info!("Global interface ready! Preparing hardware");

            match interfaces::global::Interface::wait_until_ready(
                self.interface_memory.as_ref().unwrap(),
            ) {
                Ok(version) => log::info!("Firmware finished loading with version: {:x}", version),
                Err(status) => {
                    log::error!("Failed to wait for global interface to be ready: {:#?}", status);
                    return;
                }
            }

            let interface =
                interfaces::global::Interface::new(self.interface_memory.as_ref().unwrap());
            if let Err(e) =
                interface.initialize(self.interface_memory.as_mut().unwrap(), &mut self.mmio)
            {
                log::info!("Failed to initialize shared interface: {:?}", e);
                return;
            }

            self.shared_interface = Some(interface);

            self.create_group_run_read_instructions().unwrap();
        } else if status.0 != 0 {
            log::error!("Unknown JOB irq: {:#?}", status);
        }
    }

    pub fn thread_entry(
        state: DeviceState,
        mut receiver: DeviceTaskReceiver,
        interrupts: DeviceInterrupts,
    ) {
        let port = zx::Port::create_with_opts(zx::PortOptions::BIND_TO_INTERRUPT);
        let mut executor = fuchsia_async::LocalExecutorBuilder::new().port(port).build();

        let state = Arc::new(RefCell::new(state));

        // Run the handling of the IRQ.
        let scope = fuchsia_async::Scope::new();
        let state_clone = state.clone();
        let _ = scope.spawn_local(async move {
            futures::join!(
                Self::gpu_irq_task(state_clone.clone(), interrupts.gpu),
                Self::mmu_irq_task(state_clone.clone(), interrupts.mmu),
                Self::job_irq_task(state_clone, interrupts.job)
            );
        });
        // Run the handling of tasks.
        let state_clone = state.clone();
        let result = executor.run_singlethreaded(async move {
            while let Some(task) = receiver.next().await {
                DeviceTask::handle(task, state_clone.clone());
                if state_clone.borrow().should_shutdown {
                    log::info!("Exiting device thread");
                    return Ok(());
                }
            }
            return Err(());
        });

        if result.is_err() {
            log::error!("device thread exited abnormally");
        }
    }

    pub fn load_firmware(&mut self, firmware: Vec<firmware::Section>) -> Result<(), zx::Status> {
        // Make sure our hardware blocks are off.
        hardware::initialize(&mut self.mmio)?;

        // Find our shared section and map it so we can read it later.
        self.interface_memory = firmware.iter().find_map(|section| {
            if section.flags.shared() == 0 {
                return None;
            }
            section
                .data
                .map(0, section.data.size(), zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE)
                .log_err("Failed to map shared section")
                .ok()
        });

        // Map the firmware into an address space, and bind it to AS 0.
        let address_space = firmware_to_address_space(&*self.bus_mapper, &firmware)?;
        address_space.bind_to_hardware(&mut self.mmio, 0)?;

        // Force all writes to be seen before kicking the hardware.
        crate::barriers::write();

        // Go hardware go!
        crate::regs::McuControl::auto().write(&mut self.mmio);

        self.address_spaces.push(address_space);
        self.firmware = firmware;
        Ok(())
    }

    fn suspend_size(&self) -> usize {
        let interface = self.shared_interface.as_ref().unwrap();
        let control = interface.groups[0].read_control(self.interface_memory.as_ref().unwrap());
        utils::round_up_to_page_size(control.suspend_size as usize)
    }

    fn create_group_run_read_instructions(&mut self) -> Result<(), zx::Status> {
        let suspend_size = self.suspend_size();
        self.groups.push(context::Group::new(
            &*self.bus_mapper,
            &mut self.address_spaces[0],
            1,
            utils::PAGE_SIZE * 16,
            suspend_size,
        )?);
        let group = &mut self.groups[0];

        let (_, buffer_mapping, buffer_pin) =
            mem::allocate_map_pin(utils::PAGE_SIZE, &*self.bus_mapper).unwrap();
        let mut access_flags = crate::address_space::AccessFlags(0);
        access_flags.set_attribute_slot(regs::MemoryAttributeSlot::NonCacheable as u64);
        access_flags.set_access_flag_no_exec(1);
        access_flags.set_access_flag_read(1);

        let buffer_gpu_address = group.address_space.insert_buffer_auto_address(
            buffer_pin,
            &access_flags,
            &*self.bus_mapper,
        )?;

        group
            .bind(1, crate::context::FirmwareId(1), &mut self.mmio)
            .log_err("Failed to bind group")?;
        group
            .configure_interfaces(
                self.interface_memory.as_mut().unwrap(),
                &mut self.mmio,
                self.shared_interface.as_mut().unwrap(),
            )
            .log_err("Failed to program firmware")?;

        let group = &mut self.groups[0];
        group.ringbuffers[0]
            .add_instructions(&hardware::ringbuffer_instructions_store_data_for_test(
                &buffer_gpu_address,
                0xcafecafe,
            ))
            .unwrap();

        group.ringbuffers[0].kick(&mut self.mmio).unwrap();

        // We are sleeping here instead of waiting on an IRQ, this could be updated.
        std::thread::sleep(std::time::Duration::from_millis(100));

        log::info!("GPU instructions received: {:x}", buffer_mapping.read32(0));
        Ok(())
    }
}

fn firmware_to_address_space(
    bus_mapper: &dyn BusMapper,
    firmware: &Vec<firmware::Section>,
) -> Result<AddressSpace, zx::Status> {
    // Our address space is picked as noncoherent to be conservative.
    // TODO(https://fxbug.dev/498573259): This value should come from the hardware ideally.
    let mut address_space = AddressSpace::new(Coherency::NonCoherent, bus_mapper)
        .log_err("Failed to create address space")?;
    for section in firmware {
        let pinned = section
            .data
            .pin(bus_mapper, 0, section.data.size(), section.flags.into_bti_options())
            .log_err("Failed to pin buffer")?;
        address_space
            .insert_buffer(
                mem::GpuAddress(section.virtual_address.start as u64),
                pinned,
                &section.flags.into_address_space_flags(),
                bus_mapper,
            )
            .log_err("Failed to insert buffer")?;
    }
    Ok(address_space)
}

pub struct DeviceInterrupts {
    pub job: zx::Interrupt,
    pub mmu: zx::Interrupt,
    pub gpu: zx::Interrupt,
}

impl DeviceInterrupts {
    pub async fn new(pdev: &fidl_next::Client<fpdev::Device>) -> Result<Self, DriverError> {
        const JOB_IRQ_ID: u32 = 0;
        const MMU_IRQ_ID: u32 = 1;
        const GPU_IRQ_ID: u32 = 2;
        Ok(DeviceInterrupts {
            job: pdev
                .get_interrupt_by_id(JOB_IRQ_ID, 0)
                .await?
                .map_err(|s| s.err().unwrap_or(zx::Status::INTERNAL))?
                .irq,
            mmu: pdev
                .get_interrupt_by_id(MMU_IRQ_ID, 0)
                .await?
                .map_err(|s| s.err().unwrap_or(zx::Status::INTERNAL))?
                .irq,
            gpu: pdev
                .get_interrupt_by_id(GPU_IRQ_ID, 0)
                .await?
                .map_err(|s| s.err().unwrap_or(zx::Status::INTERNAL))?
                .irq,
        })
    }
}

pub struct DeviceClient {
    pub device_task_sender: DeviceTaskSender,
    // This has Mutex<Option<>> so it can be taken and joined.
    pub device_thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl DeviceClient {
    pub fn new(state: DeviceState, interrupts: DeviceInterrupts) -> DeviceClient {
        let (device_task_sender, receiver) = DeviceTaskSender::new();

        let device_thread = std::thread::Builder::new()
            .name("DeviceThread".to_string())
            .spawn(move || {
                DeviceState::thread_entry(state, receiver, interrupts);
            })
            .unwrap();

        DeviceClient { device_task_sender, device_thread: Mutex::new(Some(device_thread)) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mem::tests::FakeBusMapper;

    #[fuchsia::test]
    fn test_firmware_to_address_space() {
        let Ok(file) = std::fs::File::open("/pkg/data/firmware.bin") else {
            log::warn!("Skipping test because no firmware file");
            return;
        };
        let vmo = fdio::get_vmo_copy_from_file(&file).unwrap();
        let firmware_map = crate::mem::MappedMemory::new(
            &vmo,
            0,
            vmo.get_size().unwrap() as usize,
            zx::VmarFlags::PERM_READ,
        )
        .unwrap();
        let firmware = firmware::parse_firmware(firmware_map.as_u8()).unwrap();

        let bus_mapper = FakeBusMapper::new(0x1000);
        let _address_space = firmware_to_address_space(&bus_mapper, &firmware).unwrap();
    }
}
