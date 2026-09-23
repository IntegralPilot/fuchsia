// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A fake PCI device server for testing Fuchsia drivers.

#![deny(missing_docs)]

use arrayvec::ArrayVec;
use core::fmt::Debug;
use fidl_fuchsia_driver_framework as fdf;
use fidl_next::{Request, Responder};
use fidl_next_fuchsia_hardware_pci::{self as fpci, DeviceServerHandler};
use fuchsia_async as fasync;
use fuchsia_component::server::{ServiceFs, ServiceObjTrait};
use fuchsia_sync::Mutex;
use std::collections::HashMap;
use std::mem::size_of;
use std::sync::Arc;

const BASE_CONFIG_SIZE: usize = 256;
const CONFIG_HEADER_SIZE: usize = 64;
const MSI_MAX_VECTORS: u32 = 32;
const MSIX_MAX_VECTORS: u32 = 8;
const PCI_EXPRESS_CAPABILITY_SIZE: u8 = 0x3B;

// Note: IO BARs are not currently supported.
enum Bar {
    Vmo { size: u64, vmo: zx::Vmo },
}

impl Bar {
    fn to_fidl(&self, bar_id: u32) -> Result<fpci::Bar, zx::Status> {
        match self {
            Self::Vmo { size, vmo } => {
                let vmo_dup = vmo.duplicate_handle(zx::Rights::SAME_RIGHTS)?;
                Ok(fpci::Bar { bar_id, size: *size, result: fpci::BarResult::Vmo(vmo_dup) })
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Capability {
    id: fpci::CapabilityId,
    position: u8,
    size: u8,
}

struct State {
    device_info: fpci::DeviceInfo,
    bus_mastering: Option<bool>,
    config: Vec<u8>,
    bars: [Option<Bar>; fpci::MAX_BAR_COUNT as usize],
    btis: HashMap<u32, zx::Bti>,
    capabilities: Vec<Capability>,
    legacy_interrupt: Option<Arc<zx::VirtualInterrupt>>,
    msi_interrupts: ArrayVec<Arc<zx::VirtualInterrupt>, { MSI_MAX_VECTORS as usize }>,
    msix_interrupts: ArrayVec<Arc<zx::VirtualInterrupt>, { MSIX_MAX_VECTORS as usize }>,
    interrupt_mode: fpci::InterruptMode,
    requested_irq_count: u32,
    reset_count: u32,
}

impl State {
    fn new() -> Self {
        Self {
            device_info: fpci::DeviceInfo {
                vendor_id: 0,
                device_id: 0,
                base_class: 0,
                sub_class: 0,
                program_interface: 0,
                revision_id: 0,
                bus_id: 0,
                dev_id: 0,
                func_id: 0,
                padding: (),
            },
            bus_mastering: None,
            config: vec![0u8; BASE_CONFIG_SIZE],
            bars: Default::default(),
            btis: HashMap::new(),
            capabilities: Vec::new(),
            legacy_interrupt: None,
            msi_interrupts: ArrayVec::new(),
            msix_interrupts: ArrayVec::new(),
            interrupt_mode: fpci::InterruptMode::Disabled,
            requested_irq_count: 0,
            reset_count: 0,
        }
    }

    fn write_raw(&mut self, offset: usize, bytes: &[u8]) {
        self.config[offset..offset + bytes.len()].copy_from_slice(bytes);
    }

    fn config_slice(&self, offset: u16, length: usize) -> Result<&[u8], zx::Status> {
        let start = usize::from(offset);
        let end = start.checked_add(length).ok_or(zx::Status::OUT_OF_RANGE)?;
        if end > BASE_CONFIG_SIZE {
            return Err(zx::Status::OUT_OF_RANGE);
        }
        Ok(&self.config[start..end])
    }

    fn config_slice_mut(&mut self, offset: u16, length: usize) -> Result<&mut [u8], zx::Status> {
        let start = usize::from(offset);
        let end = start.checked_add(length).ok_or(zx::Status::OUT_OF_RANGE)?;
        // We only allow config to be written past the header.
        if start < CONFIG_HEADER_SIZE || end > BASE_CONFIG_SIZE {
            return Err(zx::Status::OUT_OF_RANGE);
        }
        Ok(&mut self.config[start..end])
    }

    fn read_config8(&self, offset: u16) -> Result<u8, zx::Status> {
        let slice = self.config_slice(offset, size_of::<u8>())?;
        Ok(slice[0])
    }

    fn read_config16(&self, offset: u16) -> Result<u16, zx::Status> {
        let slice = self.config_slice(offset, size_of::<u16>())?;
        Ok(u16::from_le_bytes(slice.try_into().unwrap()))
    }

    fn read_config32(&self, offset: u16) -> Result<u32, zx::Status> {
        let slice = self.config_slice(offset, size_of::<u32>())?;
        Ok(u32::from_le_bytes(slice.try_into().unwrap()))
    }

    fn write_config8(&mut self, offset: u16, value: u8) -> Result<(), zx::Status> {
        let slice = self.config_slice_mut(offset, size_of::<u8>())?;
        slice[0] = value;
        Ok(())
    }

    fn write_config16(&mut self, offset: u16, value: u16) -> Result<(), zx::Status> {
        let slice = self.config_slice_mut(offset, size_of::<u16>())?;
        slice.copy_from_slice(&value.to_le_bytes());
        Ok(())
    }

    fn write_config32(&mut self, offset: u16, value: u32) -> Result<(), zx::Status> {
        let slice = self.config_slice_mut(offset, size_of::<u32>())?;
        slice.copy_from_slice(&value.to_le_bytes());
        Ok(())
    }

    fn set_device_info(&mut self, info: fpci::DeviceInfo) {
        self.device_info = info;
        self.write_raw(u16::from(fpci::Config::VendorId).into(), &info.vendor_id.to_le_bytes());
        self.write_raw(u16::from(fpci::Config::DeviceId).into(), &info.device_id.to_le_bytes());
        self.write_raw(u16::from(fpci::Config::RevisionId).into(), &info.revision_id.to_le_bytes());
        self.write_raw(
            u16::from(fpci::Config::ClassCodeBase).into(),
            &info.base_class.to_le_bytes(),
        );
        self.write_raw(u16::from(fpci::Config::ClassCodeSub).into(), &info.sub_class.to_le_bytes());
        self.write_raw(
            u16::from(fpci::Config::ClassCodeIntr).into(),
            &info.program_interface.to_le_bytes(),
        );
    }

    fn add_capability(&mut self, capability_id: fpci::CapabilityId, position: u8, size: u8) {
        assert!(
            !matches!(capability_id, fpci::CapabilityId::UnknownOrdinal_(_)),
            "Unknown capability ID is forbidden"
        );
        let pos = usize::from(position);
        let sz = usize::from(size);
        assert!(
            pos >= CONFIG_HEADER_SIZE && pos + sz <= BASE_CONFIG_SIZE,
            "capability must fit the range [0x{CONFIG_HEADER_SIZE:X}, 0x{BASE_CONFIG_SIZE:X}]"
        );

        for cap in &self.capabilities {
            let cap_pos = usize::from(cap.position);
            let cap_sz = usize::from(cap.size);
            let overlap = (pos < cap_pos + cap_sz) && (pos + sz > cap_pos);
            assert!(!overlap, "New capability overlaps with a previous capability {cap:?}");
        }

        let next_ptr = if self.capabilities.is_empty() {
            usize::from(u16::from(fpci::Config::CapabilitiesPtr))
        } else {
            usize::from(self.capabilities.last().unwrap().position) + 1
        };

        self.write_raw(pos, &[u8::from(capability_id)]);
        self.write_raw(next_ptr, &[position]);

        self.capabilities.push(Capability { id: capability_id, position, size });
        self.capabilities.sort_by_key(|c| c.position);
    }

    fn add_vendor_capability(&mut self, position: u8, size: u8) {
        assert!(size > 2, "vendor capability must be at least size 3");
        self.add_capability(fpci::CapabilityId::Vendor, position, size);
        // Vendor capabilities store a size at the byte following the next
        // pointer.
        self.write_raw(usize::from(position) + 2, &[size]);
    }

    fn add_pci_express_capability(&mut self, position: u8) {
        self.add_capability(fpci::CapabilityId::PciExpress, position, PCI_EXPRESS_CAPABILITY_SIZE);
    }

    fn get_capabilities(&self, id: fpci::CapabilityId) -> Vec<u8> {
        let mut offsets = Vec::new();
        let mut current_offset: Option<u8> = None;

        while let Some(found_offset) = self.common_capability_search(id, current_offset) {
            offsets.push(found_offset);
            current_offset = Some(found_offset);
        }
        offsets
    }

    fn common_capability_search(&self, id: fpci::CapabilityId, offset: Option<u8>) -> Option<u8> {
        for cap in &self.capabilities {
            if offset.is_some_and(|off| cap.position <= off) {
                continue;
            }
            if cap.id == id {
                return Some(cap.position);
            }
        }
        None
    }

    fn all_mapped_interrupts_freed(&self) -> bool {
        let legacy = self.legacy_interrupt.as_ref().into_iter();
        for irq in legacy.chain(self.msi_interrupts.iter()).chain(self.msix_interrupts.iter()) {
            let info = irq.count_info().expect("get irq info");
            // We only expect the interrupt handle FakePci created to still be
            // in use.
            if info.handle_count > 1 {
                return false;
            }
        }
        true
    }

    fn get_interrupt_modes(&self) -> fpci::InterruptModes {
        let has_legacy = self.legacy_interrupt.is_some();
        let msi_count = if self.msi_interrupts.is_empty() {
            0
        } else if self.msi_interrupts.len() == 1 {
            1
        } else {
            let len = u8::try_from(self.msi_interrupts.len()).unwrap();
            1 << (7 - len.leading_zeros())
        };
        let msix_count = u16::try_from(self.msix_interrupts.len()).unwrap();
        fpci::InterruptModes { has_legacy, msi_count, msix_count }
    }

    fn set_interrupt_mode(
        &mut self,
        mode: fpci::InterruptMode,
        requested_irq_count: u32,
    ) -> Result<(), zx::Status> {
        if !self.all_mapped_interrupts_freed() {
            return Err(zx::Status::BAD_STATE);
        }
        match mode {
            fpci::InterruptMode::Disabled => {
                self.interrupt_mode = mode;
                self.requested_irq_count = 0;
                Ok(())
            }
            fpci::InterruptMode::Legacy => {
                if requested_irq_count > 1 {
                    return Err(zx::Status::INVALID_ARGS);
                }
                if self.legacy_interrupt.is_none() {
                    return Err(zx::Status::NOT_SUPPORTED);
                }
                self.interrupt_mode = mode;
                self.requested_irq_count = 1;
                Ok(())
            }
            fpci::InterruptMode::Msi => {
                if self.msi_interrupts.is_empty() {
                    return Err(zx::Status::NOT_SUPPORTED);
                }
                if requested_irq_count == 0
                    || !requested_irq_count.is_power_of_two()
                    || requested_irq_count > MSI_MAX_VECTORS
                {
                    return Err(zx::Status::INVALID_ARGS);
                }
                if u32::try_from(self.msi_interrupts.len()).unwrap() < requested_irq_count {
                    return Err(zx::Status::INVALID_ARGS);
                }
                self.interrupt_mode = mode;
                self.requested_irq_count = requested_irq_count;
                Ok(())
            }
            fpci::InterruptMode::MsiX => {
                if self.msix_interrupts.is_empty() {
                    return Err(zx::Status::NOT_SUPPORTED);
                }
                if requested_irq_count == 0 || requested_irq_count > MSIX_MAX_VECTORS {
                    return Err(zx::Status::INVALID_ARGS);
                }
                if u32::try_from(self.msix_interrupts.len()).unwrap() < requested_irq_count {
                    return Err(zx::Status::INVALID_ARGS);
                }
                self.interrupt_mode = mode;
                self.requested_irq_count = requested_irq_count;
                Ok(())
            }
            _ => Err(zx::Status::NOT_SUPPORTED),
        }
    }

    fn map_interrupt(&self, which_irq: u32) -> Result<zx::Interrupt, zx::Status> {
        match self.interrupt_mode {
            fpci::InterruptMode::Legacy => {
                if which_irq > 0 {
                    return Err(zx::Status::INVALID_ARGS);
                }
                let irq = self.legacy_interrupt.as_ref().ok_or(zx::Status::BAD_STATE)?;
                Ok(zx::Interrupt::from(
                    irq.duplicate_handle(zx::Rights::SAME_RIGHTS)
                        .expect("duplicate irq")
                        .into_handle(),
                ))
            }
            fpci::InterruptMode::Msi => {
                let which = usize::try_from(which_irq).unwrap();
                if which >= usize::try_from(self.requested_irq_count).unwrap() {
                    return Err(zx::Status::INVALID_ARGS);
                }
                let irq = self.msi_interrupts.get(which).ok_or(zx::Status::INVALID_ARGS)?;
                Ok(zx::Interrupt::from(
                    irq.duplicate_handle(zx::Rights::SAME_RIGHTS)
                        .expect("duplicate irq")
                        .into_handle(),
                ))
            }
            fpci::InterruptMode::MsiX => {
                let which = usize::try_from(which_irq).unwrap();
                if which >= usize::try_from(self.requested_irq_count).unwrap() {
                    return Err(zx::Status::INVALID_ARGS);
                }
                let irq = self.msix_interrupts.get(which).ok_or(zx::Status::INVALID_ARGS)?;
                Ok(zx::Interrupt::from(
                    irq.duplicate_handle(zx::Rights::SAME_RIGHTS)
                        .expect("duplicate irq")
                        .into_handle(),
                ))
            }
            _ => Err(zx::Status::BAD_STATE),
        }
    }

    fn ack_interrupt(&self) -> Result<(), zx::Status> {
        if self.interrupt_mode == fpci::InterruptMode::Legacy {
            Ok(())
        } else {
            Err(zx::Status::BAD_STATE)
        }
    }
}

/// A fake implementation of the [`fpci::Device`] protocol for testing PCI drivers.
#[derive(Clone)]
pub struct FakePci {
    state: Arc<Mutex<State>>,
}

impl Default for FakePci {
    fn default() -> Self {
        Self::new()
    }
}

impl FakePci {
    /// Creates a new empty [`FakePci`] instance.
    pub fn new() -> Self {
        Self { state: Arc::new(Mutex::new(State::new())) }
    }

    /// Sets the [`fpci::DeviceInfo`] for the fake PCI device and populates header fields.
    pub fn set_device_info(&self, info: fpci::DeviceInfo) {
        self.state.lock().set_device_info(info);
    }

    /// Returns the current [`fpci::DeviceInfo`].
    pub fn device_info(&self) -> fpci::DeviceInfo {
        self.state.lock().device_info
    }

    /// Returns whether bus mastering has been enabled, or [`None`] if not set.
    pub fn bus_master_enabled(&self) -> Option<bool> {
        self.state.lock().bus_mastering
    }

    /// Returns the number of times the device has been reset.
    pub fn reset_count(&self) -> u32 {
        self.state.lock().reset_count
    }

    /// Configures a VMO BAR with the given ID and size using an existing [`zx::Vmo`].
    ///
    /// # Panics
    ///
    /// Panics if `bar_id` is invalid (`>= fpci::MAX_BAR_COUNT`), or if a BAR has
    /// already been set for `bar_id`.
    pub fn add_vmo_bar(&self, bar_id: u32, size: u64, vmo: zx::Vmo) {
        assert!(
            self.state
                .lock()
                .bars
                .get_mut(bar_id as usize)
                .expect("invalid bar id")
                .replace(Bar::Vmo { size, vmo })
                .is_none(),
            "{bar_id} already set"
        );
    }

    /// Creates and configures a new VMO BAR with the given ID and size.
    ///
    /// # Panics
    ///
    /// Panics if VMO creation fails, or under the same conditions as
    /// [`Self::add_vmo_bar`].
    pub fn create_vmo_bar(&self, bar_id: u32, size: u64) {
        self.add_vmo_bar(bar_id, size, zx::Vmo::create(size).unwrap());
    }

    /// Returns a duplicated handle to the [`zx::Vmo`] associated with `bar_id`, if present.
    pub fn vmo_bar(&self, bar_id: u32) -> Option<zx::Vmo> {
        let state = self.state.lock();
        let Bar::Vmo { size: _, vmo } =
            state.bars.get(bar_id as usize).expect("invalid bar id").as_ref()?;
        Some(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
    }

    /// Registers a [`zx::Bti`] handle at the given index.
    ///
    /// # Panics
    ///
    /// Panics if a BTI is already registered at `index`.
    pub fn add_bti(&self, index: u32, bti: zx::Bti) {
        assert!(self.state.lock().btis.insert(index, bti).is_none());
    }

    /// Adds a PCI capability at the specified offset in configuration space.
    ///
    /// # Panics
    ///
    /// Panics if:
    /// - `id` is an unknown capability ordinal.
    /// - `position` is in the configuration header or the capability
    ///   exceeds configuration space bounds.
    /// - The capability overlaps with a previously added capability.
    pub fn add_capability(&self, id: fpci::CapabilityId, position: u8, size: u8) {
        self.state.lock().add_capability(id, position, size);
    }

    /// Adds a vendor-specific PCI capability at the specified offset.
    ///
    /// # Panics
    ///
    /// Panics if `size <= 2`, or under the same conditions as
    /// [`Self::add_capability`].
    pub fn add_vendor_capability(&self, position: u8, size: u8) {
        self.state.lock().add_vendor_capability(position, size);
    }

    /// Adds a PCI Express capability at the specified offset.
    ///
    /// # Panics
    ///
    /// Panics under the same conditions as [`Self::add_capability`].
    pub fn add_pci_express_capability(&self, position: u8) {
        self.state.lock().add_pci_express_capability(position);
    }

    /// Adds a legacy virtual interrupt and returns its [`zx::VirtualInterrupt`].
    ///
    /// # Panics
    ///
    /// Panics if creation of the virtual interrupt fails, or if a legacy
    /// interrupt has already been added.
    pub fn add_legacy_interrupt(&self) -> Arc<zx::VirtualInterrupt> {
        let virq = Arc::new(
            zx::VirtualInterrupt::create_virtual().expect("Failed to create virtual interrupt"),
        );
        let mut state = self.state.lock();
        assert!(state.legacy_interrupt.is_none(), "Legacy interrupt already added");
        state.legacy_interrupt = Some(virq.clone());
        virq
    }

    /// Adds an MSI virtual interrupt and returns its [`zx::VirtualInterrupt`].
    ///
    /// Subsequent calls populate interrupts at incrementing indices.
    ///
    /// # Panics
    ///
    /// Panics if creation of the virtual interrupt fails, or if the number of
    /// MSI interrupts exceeds the protocol maximum.
    pub fn add_msi_interrupt(&self) -> Arc<zx::VirtualInterrupt> {
        let virq = Arc::new(
            zx::VirtualInterrupt::create_virtual().expect("Failed to create virtual interrupt"),
        );
        let mut state = self.state.lock();
        state.msi_interrupts.push(virq.clone());
        virq
    }

    /// Adds an MSI-X virtual interrupt and returns its [`zx::VirtualInterrupt`].
    ///
    /// Subsequent calls populate interrupts at incrementing indices.
    ///
    /// # Panics
    ///
    /// Panics if creation of the virtual interrupt fails, or if the number of
    /// MSI-X interrupts exceeds the protocol maximum.
    pub fn add_msix_interrupt(&self) -> Arc<zx::VirtualInterrupt> {
        let virq = Arc::new(
            zx::VirtualInterrupt::create_virtual().expect("Failed to create virtual interrupt"),
        );
        let mut state = self.state.lock();
        state.msix_interrupts.push(virq.clone());
        virq
    }

    /// Returns the currently configured [`fpci::InterruptMode`].
    pub fn interrupt_mode(&self) -> fpci::InterruptMode {
        self.state.lock().interrupt_mode
    }

    /// Returns the number of requested interrupt vectors.
    pub fn requested_irq_count(&self) -> u32 {
        self.state.lock().requested_irq_count
    }

    /// Reads an 8-bit value from configuration space at `offset`.
    ///
    /// # Panics
    ///
    /// Panics if the read exceeds configuration space bounds.
    pub fn read_config8(&self, offset: u16) -> u8 {
        self.state.lock().read_config8(offset).expect("read_config8 failed")
    }

    /// Reads a 16-bit value from configuration space at `offset`.
    ///
    /// # Panics
    ///
    /// Panics if the read exceeds configuration space bounds.
    pub fn read_config16(&self, offset: u16) -> u16 {
        self.state.lock().read_config16(offset).expect("read_config16 failed")
    }

    /// Reads a 32-bit value from configuration space at `offset`.
    ///
    /// # Panics
    ///
    /// Panics if the read exceeds configuration space bounds.
    pub fn read_config32(&self, offset: u16) -> u32 {
        self.state.lock().read_config32(offset).expect("read_config32 failed")
    }

    /// Writes an 8-bit `value` to configuration space at `offset`.
    ///
    /// # Panics
    ///
    /// Panics if `offset` is in the configuration header or the write
    /// exceeds configuration space bounds.
    pub fn write_config8(&self, offset: u16, value: u8) {
        self.state.lock().write_config8(offset, value).expect("write_config8 failed");
    }

    /// Writes a 16-bit `value` to configuration space at `offset`.
    ///
    /// # Panics
    ///
    /// Panics if `offset` is in the configuration header or the write
    /// exceeds configuration space bounds.
    pub fn write_config16(&self, offset: u16, value: u16) {
        self.state.lock().write_config16(offset, value).expect("write_config16 failed");
    }

    /// Writes a 32-bit `value` to configuration space at `offset`.
    ///
    /// # Panics
    ///
    /// Panics if `offset` is in the configuration header or the write
    /// exceeds configuration space bounds.
    pub fn write_config32(&self, offset: u16, value: u32) {
        self.state.lock().write_config32(offset, value).expect("write_config32 failed");
    }

    /// Offers the [`fpci::Service`] to `service_fs` and returns the [`fdf::Offer`].
    pub fn serve<O: ServiceObjTrait>(
        &self,
        service_fs: &mut ServiceFs<O>,
        scope: fasync::ScopeHandle,
        instance_name: &str,
    ) -> fdf::Offer {
        fdf_component::ServiceOffer::<fpci::Service>::new_next()
            .add_default_named_next(
                service_fs,
                instance_name,
                Service { state: self.state.clone(), scope },
            )
            .build_zircon_offer_next()
    }
}

struct Service {
    state: Arc<Mutex<State>>,
    scope: fasync::ScopeHandle,
}

impl fpci::ServiceHandler for Service {
    fn device(&self, server_end: fidl_next::ServerEnd<fpci::Device>) {
        server_end.spawn_on(Server { state: self.state.clone() }, &self.scope);
    }
}

struct Server {
    state: Arc<Mutex<State>>,
}

impl DeviceServerHandler for Server {
    async fn get_device_info(&mut self, responder: Responder<fpci::device::GetDeviceInfo>) {
        let info = self.state.lock().device_info;
        responder.respond(&info).await.expect_respond("get_device_info");
    }

    async fn get_bar(
        &mut self,
        request: Request<fpci::device::GetBar>,
        responder: Responder<fpci::device::GetBar>,
    ) {
        let bar_id = request.payload().bar_id;
        let result = {
            let state = self.state.lock();
            match state.bars.get(bar_id as usize).as_ref() {
                None => Err(zx::Status::INVALID_ARGS),
                Some(None) => Err(zx::Status::NOT_FOUND),
                Some(Some(bar)) => bar.to_fidl(bar_id),
            }
        };
        match result {
            Ok(bar) => {
                responder.respond(bar).await.expect_respond("get_bar");
            }
            Err(e) => {
                responder.respond_err(e).await.expect_respond("get_bar");
            }
        }
    }

    async fn set_bus_mastering(
        &mut self,
        request: Request<fpci::device::SetBusMastering>,
        responder: Responder<fpci::device::SetBusMastering>,
    ) {
        self.state.lock().bus_mastering = Some(request.payload().enabled);
        responder.respond(()).await.expect_respond("set_bus_mastering");
    }

    async fn reset_device(&mut self, responder: Responder<fpci::device::ResetDevice>) {
        self.state.lock().reset_count += 1;
        responder.respond(()).await.expect_respond("reset_device");
    }

    async fn ack_interrupt(&mut self, responder: Responder<fpci::device::AckInterrupt>) {
        let result = self.state.lock().ack_interrupt();
        match result {
            Ok(()) => {
                responder.respond(()).await.expect_respond("ack_interrupt");
            }
            Err(e) => {
                responder.respond_err(e).await.expect_respond("ack_interrupt");
            }
        }
    }

    async fn map_interrupt(
        &mut self,
        request: Request<fpci::device::MapInterrupt>,
        responder: Responder<fpci::device::MapInterrupt>,
    ) {
        let which_irq = request.payload().which_irq;
        let result = self.state.lock().map_interrupt(which_irq);
        match result {
            Ok(interrupt) => {
                responder.respond(interrupt).await.expect_respond("map_interrupt");
            }
            Err(e) => {
                responder.respond_err(e).await.expect_respond("map_interrupt");
            }
        }
    }

    async fn get_interrupt_modes(&mut self, responder: Responder<fpci::device::GetInterruptModes>) {
        let modes = self.state.lock().get_interrupt_modes();
        responder.respond(&modes).await.expect_respond("get_interrupt_modes");
    }

    async fn set_interrupt_mode(
        &mut self,
        request: Request<fpci::device::SetInterruptMode>,
        responder: Responder<fpci::device::SetInterruptMode>,
    ) {
        let payload = request.payload();
        let result =
            self.state.lock().set_interrupt_mode(payload.mode, payload.requested_irq_count);
        match result {
            Ok(()) => {
                responder.respond(()).await.expect_respond("set_interrupt_mode");
            }
            Err(e) => {
                responder.respond_err(e).await.expect_respond("set_interrupt_mode");
            }
        }
    }

    async fn read_config8(
        &mut self,
        request: Request<fpci::device::ReadConfig8>,
        responder: Responder<fpci::device::ReadConfig8>,
    ) {
        let res = self.state.lock().read_config8(request.payload().offset);
        match res {
            Ok(val) => {
                responder.respond(val).await.expect_respond("read_config8");
            }
            Err(e) => {
                responder.respond_err(e).await.expect_respond("read_config8");
            }
        }
    }

    async fn read_config16(
        &mut self,
        request: Request<fpci::device::ReadConfig16>,
        responder: Responder<fpci::device::ReadConfig16>,
    ) {
        let res = self.state.lock().read_config16(request.payload().offset);
        match res {
            Ok(val) => {
                responder.respond(val).await.expect_respond("read_config16");
            }
            Err(e) => {
                responder.respond_err(e).await.expect_respond("read_config16");
            }
        }
    }

    async fn read_config32(
        &mut self,
        request: Request<fpci::device::ReadConfig32>,
        responder: Responder<fpci::device::ReadConfig32>,
    ) {
        let res = self.state.lock().read_config32(request.payload().offset);
        match res {
            Ok(val) => {
                responder.respond(val).await.expect_respond("read_config32");
            }
            Err(e) => {
                responder.respond_err(e).await.expect_respond("read_config32");
            }
        }
    }

    async fn write_config8(
        &mut self,
        request: Request<fpci::device::WriteConfig8>,
        responder: Responder<fpci::device::WriteConfig8>,
    ) {
        let payload = request.payload();
        let res = self.state.lock().write_config8(payload.offset, payload.value);
        match res {
            Ok(()) => {
                responder.respond(()).await.expect_respond("write_config8");
            }
            Err(e) => {
                responder.respond_err(e).await.expect_respond("write_config8");
            }
        }
    }

    async fn write_config16(
        &mut self,
        request: Request<fpci::device::WriteConfig16>,
        responder: Responder<fpci::device::WriteConfig16>,
    ) {
        let payload = request.payload();
        let res = self.state.lock().write_config16(payload.offset, payload.value);
        match res {
            Ok(()) => {
                responder.respond(()).await.expect_respond("write_config16");
            }
            Err(e) => {
                responder.respond_err(e).await.expect_respond("write_config16");
            }
        }
    }

    async fn write_config32(
        &mut self,
        request: Request<fpci::device::WriteConfig32>,
        responder: Responder<fpci::device::WriteConfig32>,
    ) {
        let payload = request.payload();
        let res = self.state.lock().write_config32(payload.offset, payload.value);
        match res {
            Ok(()) => {
                responder.respond(()).await.expect_respond("write_config32");
            }
            Err(e) => {
                responder.respond_err(e).await.expect_respond("write_config32");
            }
        }
    }

    async fn get_capabilities(
        &mut self,
        request: Request<fpci::device::GetCapabilities>,
        responder: Responder<fpci::device::GetCapabilities>,
    ) {
        let id = request.payload().id;
        let offsets = self.state.lock().get_capabilities(id);
        responder.respond(&offsets).await.expect_respond("get_capabilities");
    }

    async fn get_extended_capabilities(
        &mut self,
        _request: Request<fpci::device::GetExtendedCapabilities>,
        _responder: Responder<fpci::device::GetExtendedCapabilities>,
    ) {
        unimplemented!()
    }

    async fn get_bti(
        &mut self,
        request: Request<fpci::device::GetBti>,
        responder: Responder<fpci::device::GetBti>,
    ) {
        let index = request.payload().index;
        let result = {
            let state = self.state.lock();
            if let Some(bti) = state.btis.get(&index) {
                bti.duplicate_handle(zx::Rights::SAME_RIGHTS)
            } else {
                Err(zx::Status::NOT_FOUND)
            }
        };
        match result {
            Ok(bti) => {
                responder.respond(bti).await.expect_respond("get_bti");
            }
            Err(e) => {
                responder.respond_err(e).await.expect_respond("get_bti");
            }
        }
    }
}

trait RespondExt {
    fn expect_respond(self, msg: &str);
}

impl<E: Debug> RespondExt for Result<(), fidl_next::Error<E>> {
    fn expect_respond(self, msg: &str) {
        self.unwrap_or_else(|e: fidl_next::Error<_>| {
            // Allow some errors to pass, panic on everything else.
            match e {
                fidl_next::Error::Protocol(fidl_next::ProtocolError::PeerClosed) => {}
                e => {
                    panic!("responding {msg}: {e:?}")
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fake_bti::FakeBti;
    use fidl_next::fuchsia::create_channel;
    use fixture::fixture;

    async fn run_test_with_fake<F, Fut>(_test_name: &str, test_func: F)
    where
        F: FnOnce(fidl_next::Client<fpci::Device>, FakePci) -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        let fake_pci = FakePci::new();

        let (client_end, server_end) = create_channel::<fpci::Device>();
        let server = Server { state: fake_pci.state.clone() };
        let scope = fasync::Scope::new_with_name("test");
        server_end.spawn_on(server, &scope);

        let client = client_end.spawn();
        test_func(client, fake_pci).await;
    }

    #[fixture(run_test_with_fake)]
    #[fuchsia::test]
    async fn test_get_device_info(client: fidl_next::Client<fpci::Device>, fake_pci: FakePci) {
        let expected_info = fpci::DeviceInfo {
            vendor_id: 0x1234,
            device_id: 0x5678,
            base_class: 0x01,
            sub_class: 0x02,
            program_interface: 0x03,
            revision_id: 0x04,
            bus_id: 1,
            dev_id: 2,
            func_id: 3,
            padding: (),
        };
        fake_pci.set_device_info(expected_info);

        let res = client.get_device_info().await.expect("Failed to send get_device_info request");
        assert_eq!(res.info, expected_info);

        // Verify that config header registers were updated
        assert_eq!(
            fake_pci.read_config16(u16::from(fpci::Config::VendorId)),
            expected_info.vendor_id
        );
        assert_eq!(
            fake_pci.read_config16(u16::from(fpci::Config::DeviceId)),
            expected_info.device_id
        );
        assert_eq!(
            fake_pci.read_config8(u16::from(fpci::Config::RevisionId)),
            expected_info.revision_id
        );
        assert_eq!(
            fake_pci.read_config8(u16::from(fpci::Config::ClassCodeBase)),
            expected_info.base_class
        );
        assert_eq!(
            fake_pci.read_config8(u16::from(fpci::Config::ClassCodeSub)),
            expected_info.sub_class
        );
        assert_eq!(
            fake_pci.read_config8(u16::from(fpci::Config::ClassCodeIntr)),
            expected_info.program_interface
        );
    }

    #[fixture(run_test_with_fake)]
    #[fuchsia::test]
    async fn test_get_vmo_bar(client: fidl_next::Client<fpci::Device>, fake_pci: FakePci) {
        let bar_id = 0;
        let bar_size = 4096;
        let vmo = zx::Vmo::create(bar_size).expect("Failed to create VMO");
        let expected_koid = vmo.koid().expect("Failed to get VMO koid");
        fake_pci.add_vmo_bar(bar_id, bar_size, vmo);

        let bar_res = client
            .get_bar(bar_id)
            .await
            .expect("Failed to send get_bar request")
            .expect("get_bar returned error status");
        let fpci::Bar { bar_id: actual_bar_id, size, result } = bar_res.result;
        assert_eq!(actual_bar_id, bar_id);
        assert_eq!(size, bar_size);

        let fpci::BarResult::Vmo(returned_vmo) = result else {
            panic!("Expected VMO bar result");
        };
        assert_eq!(returned_vmo.koid().expect("Failed to get returned VMO koid"), expected_koid);

        let non_existent_bar_id = 1;
        let err_res = client
            .get_bar(non_existent_bar_id)
            .await
            .expect("Failed to send get_bar request for non-existent BAR");
        assert_eq!(
            err_res.expect_err("Expected NOT_FOUND for non-existent BAR"),
            Err(zx::Status::NOT_FOUND)
        );
    }

    #[fixture(run_test_with_fake)]
    #[fuchsia::test]
    async fn test_set_bus_mastering(client: fidl_next::Client<fpci::Device>, fake_pci: FakePci) {
        assert_eq!(fake_pci.bus_master_enabled(), None);

        let res =
            client.set_bus_mastering(true).await.expect("Failed to send set_bus_mastering request");
        assert!(res.is_ok());
        assert_eq!(fake_pci.bus_master_enabled(), Some(true));

        let res = client
            .set_bus_mastering(false)
            .await
            .expect("Failed to send set_bus_mastering request");
        assert!(res.is_ok());
        assert_eq!(fake_pci.bus_master_enabled(), Some(false));
    }

    #[fixture(run_test_with_fake)]
    #[fuchsia::test]
    async fn test_reset_device(client: fidl_next::Client<fpci::Device>, fake_pci: FakePci) {
        assert_eq!(fake_pci.reset_count(), 0);
        let res = client.reset_device().await.expect("Failed to send reset_device request");
        assert!(res.is_ok());
        assert_eq!(fake_pci.reset_count(), 1);
        let res = client.reset_device().await.expect("Failed to send reset_device request");
        assert!(res.is_ok());
        assert_eq!(fake_pci.reset_count(), 2);
    }

    #[fixture(run_test_with_fake)]
    #[fuchsia::test]
    async fn test_interrupt_api_and_modes(
        client: fidl_next::Client<fpci::Device>,
        fake_pci: FakePci,
    ) {
        let legacy_irq = fake_pci.add_legacy_interrupt();
        let msi0 = fake_pci.add_msi_interrupt();
        let _msi1 = fake_pci.add_msi_interrupt();
        let _msi2 = fake_pci.add_msi_interrupt();
        let msix0 = fake_pci.add_msix_interrupt();

        // Get interrupt modes - msi should round down 3 -> 2
        let modes_res =
            client.get_interrupt_modes().await.expect("Failed to send get_interrupt_modes request");
        let fpci::DeviceGetInterruptModesResponse { modes } = modes_res;
        assert!(modes.has_legacy);
        assert_eq!(modes.msi_count, 2);
        assert_eq!(modes.msix_count, 1);

        // Set Legacy mode
        let res = client
            .set_interrupt_mode(fpci::InterruptMode::Legacy, 1)
            .await
            .expect("Set legacy interrupt mode failed");
        assert!(res.is_ok());
        assert_eq!(fake_pci.interrupt_mode(), fpci::InterruptMode::Legacy);

        // Ack interrupt should succeed for legacy
        let ack_res = client.ack_interrupt().await.expect("ack_interrupt failed");
        assert!(ack_res.is_ok());

        // Map legacy interrupt
        let map_res = client.map_interrupt(0).await.expect("map_interrupt failed");
        let mapped = map_res.expect("map_interrupt error status").interrupt;
        assert_eq!(mapped.koid().unwrap(), legacy_irq.koid().unwrap());

        // Setting interrupt mode while mapped handle is held should fail with BAD_STATE
        let bad_state_res = client
            .set_interrupt_mode(fpci::InterruptMode::Msi, 2)
            .await
            .expect("set_interrupt_mode failed");
        assert_eq!(bad_state_res.unwrap_err(), Err(zx::Status::BAD_STATE));

        // Drop mapped handle, now switching mode works
        std::mem::drop(mapped);
        let set_msi_res = client
            .set_interrupt_mode(fpci::InterruptMode::Msi, 2)
            .await
            .expect("set_interrupt_mode failed");
        assert!(set_msi_res.is_ok());

        // Ack interrupt should fail for MSI mode
        let ack_err = client.ack_interrupt().await.expect("ack_interrupt failed");
        assert_eq!(ack_err.unwrap_err(), Err(zx::Status::BAD_STATE));

        // Map MSI 0
        let map_msi = client.map_interrupt(0).await.expect("map_interrupt failed");
        let mapped_msi = map_msi.expect("mapped msi").interrupt;
        assert_eq!(mapped_msi.koid().unwrap(), msi0.koid().unwrap());

        // Map MSI-X
        std::mem::drop(mapped_msi);
        let set_msix_res = client
            .set_interrupt_mode(fpci::InterruptMode::MsiX, 1)
            .await
            .expect("set_interrupt_mode failed");
        assert_eq!(set_msix_res, Ok(()));

        let map_msix =
            client.map_interrupt(0).await.expect("map_interrupt failed").expect("mapped msix");
        assert_eq!(map_msix.interrupt.koid().unwrap(), msix0.koid().unwrap());
    }

    #[fixture(run_test_with_fake)]
    #[fuchsia::test]
    async fn test_config_space_bounds_and_header_protection(
        client: fidl_next::Client<fpci::Device>,
        fake_pci: FakePci,
    ) {
        // Reads in header space [0, 63] via FIDL succeed
        let r8 = client.read_config8(0).await.expect("read_config8");
        assert!(r8.is_ok());

        // Writes in header space [0, 63] via FIDL fail with OUT_OF_RANGE
        let w8_header = client.write_config8(0, 0xff).await.expect("write_config8");
        assert_eq!(w8_header.unwrap_err(), Err(zx::Status::OUT_OF_RANGE));

        let w16_header = client.write_config16(0x3e, 0xffff).await.expect("write_config16");
        assert_eq!(w16_header.unwrap_err(), Err(zx::Status::OUT_OF_RANGE));

        // Valid reads and writes in range [0x40, 0xFF]
        let w8_valid = client.write_config8(0x40, 0xab).await.expect("write_config8");
        assert!(w8_valid.is_ok());
        assert_eq!(fake_pci.read_config8(0x40), 0xab);

        let r8_valid = client.read_config8(0x40).await.expect("read_config8");
        assert_eq!(r8_valid.unwrap().value, 0xab);

        // Out of bounds past 256 bytes fails with OUT_OF_RANGE over FIDL
        let r_oob = client.read_config8(256).await.expect("read_config8");
        assert_eq!(r_oob.unwrap_err(), Err(zx::Status::OUT_OF_RANGE));

        let w_oob = client.write_config8(256, 0x12).await.expect("write_config8");
        assert_eq!(w_oob.unwrap_err(), Err(zx::Status::OUT_OF_RANGE));
    }

    #[fixture(run_test_with_fake)]
    #[fuchsia::test]
    async fn test_capabilities_chain_and_search(
        client: fidl_next::Client<fpci::Device>,
        fake_pci: FakePci,
    ) {
        let pos1 = 0x50;
        let pos2 = 0x60;
        let pos3 = 0x70;
        fake_pci.add_vendor_capability(pos1, 6);
        fake_pci.add_vendor_capability(pos2, 8);
        fake_pci.add_pci_express_capability(pos3);

        assert_eq!(fake_pci.read_config8(u16::from(fpci::Config::CapabilitiesPtr)), pos1);
        assert_eq!(fake_pci.read_config8((pos1 + 1).into()), pos2);
        assert_eq!(fake_pci.read_config8((pos2 + 1).into()), pos3);

        let res_vendor =
            client.get_capabilities(fpci::CapabilityId::Vendor).await.expect("get_capabilities");
        assert_eq!(res_vendor.offsets, vec![pos1, pos2]);

        let res_pcie = client
            .get_capabilities(fpci::CapabilityId::PciExpress)
            .await
            .expect("get_capabilities");
        assert_eq!(res_pcie.offsets, vec![pos3]);
    }

    #[fuchsia::test(logging = false)]
    #[should_panic]
    fn test_add_capability_unknown_forbidden() {
        let fake = FakePci::new();
        fake.add_capability(fpci::CapabilityId::UnknownOrdinal_(0x99), 0x50, 6);
    }

    #[fuchsia::test(logging = false)]
    #[should_panic]
    fn test_add_capability_overlap_panics() {
        let fake = FakePci::new();
        fake.add_capability(fpci::CapabilityId::Vendor, 0x50, 8);
        fake.add_capability(fpci::CapabilityId::PciExpress, 0x54, 8);
    }

    #[fuchsia::test(logging = false)]
    #[should_panic]
    fn test_fake_pci_read_oob_panics() {
        let fake = FakePci::new();
        let _: u8 = fake.read_config8(256);
    }

    #[fuchsia::test(logging = false)]
    #[should_panic]
    fn test_fake_pci_write_header_panics() {
        let fake = FakePci::new();
        fake.write_config8(0x10, 0xff);
    }

    #[fuchsia::test(logging = false)]
    fn test_vmo_bar_readback_and_nonexistent() {
        let fake = FakePci::new();
        assert!(fake.vmo_bar(0).is_none());

        fake.create_vmo_bar(0, 4096);
        let vmo = fake.vmo_bar(0).expect("Expected VMO for bar 0");
        assert_eq!(vmo.get_size().unwrap(), 4096);

        assert!(fake.vmo_bar(1).is_none());
    }

    #[fuchsia::test(logging = false)]
    #[should_panic]
    fn test_duplicate_add_vmo_bar() {
        let fake = FakePci::new();
        fake.create_vmo_bar(0, 4096);
        fake.create_vmo_bar(0, 4096);
    }

    #[fuchsia::test(logging = false)]
    #[should_panic]
    fn test_duplicate_add_bti() {
        let fake = FakePci::new();
        let bti1 = FakeBti::create().expect("Failed to create FakeBti");
        let bti2 = FakeBti::create().expect("Failed to create FakeBti");
        fake.add_bti(
            0,
            bti1.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("Failed to duplicate BTI"),
        );
        fake.add_bti(
            0,
            bti2.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("Failed to duplicate BTI"),
        );
    }
}
