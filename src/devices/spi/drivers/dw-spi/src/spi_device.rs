// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_next::{Request, Responder};
use fidl_next_fuchsia_hardware_gpio as fgpio;
use fidl_next_fuchsia_hardware_spiimpl::{
    self, SpiImplExchangeVectorResponse, SpiImplReceiveVectorResponse,
    SpiImplUnregisterVmoResponse, spi_impl as fspi_impl,
};
use log::{debug, error, warn};
use mmio::Register;
use mmio::region::MmioRegion;
use mmio::vmo::VmoMemory;
use std::time::Duration;
pub(crate) mod registers;
use registers::DwSpiRegsBlock;
use zx::Status;

use fidl_next_fuchsia_hardware_sharedmemory as fsharedmemory;
use fidl_next_fuchsia_hardware_sharedmemory::natural::SharedVmoRight;
use fidl_next_fuchsia_mem as fmem;
use std::collections::HashMap;

const FIFO_SIZE: usize = 256;

pub struct RegisteredVmo {
    pub vmo: fmem::natural::Range,
    pub rights: SharedVmoRight,
}

pub struct DwSpiDevice {
    mmio: DwSpiRegsBlock<MmioRegion<VmoMemory>>,
    cs_gpio: Option<fidl_next::Client<fgpio::Gpio>>,
    interrupt: zx::Interrupt,
    registered_vmos: HashMap<u32, RegisteredVmo>,
    loopback_registered_vmos: HashMap<u32, RegisteredVmo>,
}

impl DwSpiDevice {
    pub fn new(
        mmio: MmioRegion<VmoMemory>,
        cs_gpio: Option<fidl_next::Client<fgpio::Gpio>>,
        interrupt: zx::Interrupt,
    ) -> Self {
        DwSpiDevice {
            mmio: DwSpiRegsBlock { mmio },
            cs_gpio,
            interrupt,
            registered_vmos: HashMap::new(),
            loopback_registered_vmos: HashMap::new(),
        }
    }

    fn set_baud_rate(
        &mut self,
        parent_clock_hz: u64,
        max_bus_clock_hz: u64,
        rx_sample_delay_ns: u64,
    ) -> Result<(), Status> {
        // Round the divider up to avoid overclocking.
        let Some(numerator) = parent_clock_hz.checked_add(max_bus_clock_hz - 1) else {
            error!(
                "Unsupported max bus clock {max_bus_clock_hz} for parent clock rate {parent_clock_hz}"
            );
            return Err(Status::INVALID_ARGS);
        };

        let divider = numerator / max_bus_clock_hz;
        if divider >= 0xffff {
            error!(
                "Unsupported max bus clock {max_bus_clock_hz} for parent clock rate {parent_clock_hz}"
            );
            return Err(Status::INVALID_ARGS);
        }

        // The divider must be even.
        let divider = if divider % 2 == 0 { divider } else { divider + 1 };

        self.mmio.baudr_mut().write({
            let mut baudr = registers::Baudr::from_raw(0);
            baudr.set_sckdv(divider as u32);
            baudr
        });

        // Convert the RX delay from nanoseconds to parent clock cycles.
        let Some(numerator) = rx_sample_delay_ns.checked_mul(parent_clock_hz) else {
            error!(
                "Unsupported RX delay {rx_sample_delay_ns} for parent clock rate {parent_clock_hz}"
            );
            return Err(Status::INVALID_ARGS);
        };

        let rx_sample_delay = Duration::from_nanos(numerator);
        let rx_sample_delay_clocks = rx_sample_delay.as_secs();
        // Verify that the clock count fits in the register, and that the conversion from
        // nanoseconds to clock cycles did not result in rounding.
        if rx_sample_delay_clocks > 0xff || rx_sample_delay.subsec_nanos() != 0 {
            error!(
                "Unsupported RX delay {rx_sample_delay_ns} for parent clock rate {parent_clock_hz}"
            );
            return Err(Status::INVALID_ARGS);
        }

        self.mmio.rx_sample_dly_mut().write({
            let mut rx_sample_dly = registers::RxSampleDly::from_raw(0);
            rx_sample_dly.set_rsd(rx_sample_delay_clocks as u32);
            rx_sample_dly
        });

        Ok(())
    }

    pub fn init_registers(
        &mut self,
        parent_clock_hz: u64,
        max_bus_clock_hz: u64,
        rx_sample_delay_ns: u64,
    ) -> Result<(), Status> {
        self.mmio.ssi_enr_mut().write(registers::SsiEnr::from_raw(0));

        self.mmio.ctrlr0_mut().write({
            let mut ctrlr0 = registers::CtrlR0::from_raw(0);
            ctrlr0.set_spi_frf(0); // Standard SPI
            ctrlr0.set_frf(0); // Motorola SPI
            ctrlr0.set_dfs(7); // 8-bit (values 3-15 correspond to 4-16 bits, so 7 means 8 bits)
            ctrlr0.set_tmod(0); // Transmit & Receive
            ctrlr0
        });

        if max_bus_clock_hz > 0 {
            self.set_baud_rate(parent_clock_hz, max_bus_clock_hz, rx_sample_delay_ns)?;
        } else {
            warn!("Max bus clock rate reported to be zero, skipping baud rate initialization");
        }

        // Mask all interrupts initially in IMR
        self.mmio.imr_mut().write(registers::Imr::from_raw(0));

        // Configure the controller to interrupt us when the TX FIFO is half empty.
        self.mmio.txftlr_mut().write({
            let mut txftlr = registers::Txftlr::from_raw(0);
            txftlr.set_tft((FIFO_SIZE / 2).try_into().unwrap());
            txftlr
        });

        // Enable SSI
        self.mmio.ssi_enr_mut().write({
            let mut ssi_enr = registers::SsiEnr::from_raw(0);
            ssi_enr.set_ssi_en(true);
            ssi_enr
        });

        Ok(())
    }

    fn exchange_pio_loop(
        &mut self,
        mut txdata: &[u8],
        rx: bool,
        size: usize,
    ) -> Result<Vec<u8>, Status> {
        // Don't unmask RXFIM since we know there won't be any RX data to start.
        self.mmio.imr_mut().write({
            let mut imr = registers::Imr::from_raw(0);
            imr.set_rxoim(true);
            imr.set_rxuim(true);
            imr.set_txoim(true);
            imr.set_txeim(true);
            imr
        });

        if self.mmio.sr().read().rfne() {
            warn!("RX FIFO is not empty before starting transfer");
        }

        let mut tx_remaining = size;
        let mut rx_remaining = size;

        let mut rxdata = Vec::<u8>::with_capacity(if rx { size } else { 0 });

        loop {
            let isr = self.mmio.isr().read();

            debug!("Interrupt {:#02x}: TX: {tx_remaining} RX: {rx_remaining}", isr.to_raw());

            if isr.txois() || isr.rxuis() || isr.rxois() {
                warn!("Unexpected interrupt {:#02x}", isr.to_raw());
                return Err(Status::IO);
            }
            if !isr.txeis() && !isr.rxfis() {
                warn!("Spurious interrupt {:#02x}", isr.to_raw());
            }

            // The controller may still be draining the TX FIFO and filling the RX FIFO while we're
            // handling this interrupt. Reading TX words first prevents us from accidentally
            // overflowing the RX FIFO due to the RX count increasing after we've read it.
            let tx_words = self.mmio.txflr().read().txtfl() as usize;
            let rx_words = self.mmio.rxflr().read().rxtfl() as usize;

            assert!(tx_words <= FIFO_SIZE);
            // Fill the TX FIFO up to FIFO_SIZE - 1. If a byte is already in the TX shift register,
            // filling up to the maximum could cause an RX FIFO overflow.
            let tx_free = (FIFO_SIZE - tx_words).saturating_sub(1);

            debug!("  RX words {rx_words}, TX words {tx_words} (free {tx_free})");

            // Drain the RX FIFO first. If it's full, writing to the TX FIFO first will overflow it.
            let transfer_size = std::cmp::min(rx_remaining, rx_words);
            for _ in 0..transfer_size {
                let data = self.mmio.dr0().read().dr() as u8;
                if rx {
                    rxdata.push(data);
                }
            }

            rx_remaining -= transfer_size;

            self.mmio.rxftlr_mut().write({
                // If there are FIFO_SIZE or fewer bytes left to receive, we can configure the
                // controller to wait for the FIFO to be completely full before interrupting us.
                // Otherwise, have it interrupt us when the FIFO is half full.
                let fifo_level =
                    if rx_remaining <= FIFO_SIZE { rx_remaining.max(1) } else { FIFO_SIZE / 2 };
                let mut rxftlr = registers::Rxftlr::from_raw(0);
                rxftlr.set_rft((fifo_level - 1).try_into().unwrap());
                debug!("  New RXFTLR: {:#02x}", rxftlr.to_raw());
                rxftlr
            });

            // Next fill the TX FIFO.
            let transfer_size = std::cmp::min(tx_remaining, tx_free);
            for i in 0..transfer_size {
                let data = if txdata.len() > 0 { txdata[i] } else { 0xFF };
                self.mmio.dr0_mut().write(registers::Dr0::from_raw(data as u32));
            }

            tx_remaining -= transfer_size;
            if txdata.len() > 0 {
                txdata = &txdata[transfer_size..];
            }

            // Enable/disable FIFO threshold interrupts depending on how many bytes we have left to
            // transmit/receive.
            self.mmio.imr_mut().update(|imr| {
                imr.set_txeim(tx_remaining > 0);
                imr.set_rxfim(rx_remaining > 0);
                debug!("  New IMR: {:#02x}", imr.to_raw());
            });

            if tx_remaining == 0 && rx_remaining == 0 {
                break;
            }

            // TODO(https://fxbug.dev/529838127): Switch to OnInterrupt.
            self.interrupt.wait()?;
        }

        self.mmio.imr_mut().write(registers::Imr::from_raw(0));

        Ok(rxdata)
    }

    async fn set_cs_state(&self, chip_select: u32, assert: bool) -> Result<(), Status> {
        if chip_select == 0 {
            if let Some(cs_gpio) = &self.cs_gpio {
                let mode = if assert {
                    fgpio::natural::BufferMode::OutputLow
                } else {
                    fgpio::natural::BufferMode::OutputHigh
                };
                cs_gpio.set_buffer_mode(mode).wire().await.map_err(|e| {
                    error!("Failed to toggle CS: {e}");
                    Status::IO
                })?;
            }
        }
        Ok(())
    }

    async fn exchange_pio(
        &mut self,
        chip_select: u32,
        txdata: &[u8],
        rx: bool,
        size: usize,
    ) -> Result<Vec<u8>, Status> {
        if size == 0 {
            return Ok(vec![]);
        }

        assert!(txdata.len() > 0 || rx); // If there is no TX data then we must be receiving.
        assert!(txdata.len() == 0 || txdata.len() == size); // TX size must match RX size.

        let enable_loopback =
            chip_select == fidl_next_fuchsia_hardware_spiimpl::LOOPBACK_CHIP_SELECT;

        // Only one chip select is supported for now.
        if chip_select != 0 && !enable_loopback {
            return Err(Status::NOT_FOUND);
        }

        if self.mmio.ctrlr0().read().srl() != enable_loopback {
            self.mmio.ssi_enr_mut().write({
                let mut ssi_enr = registers::SsiEnr::from_raw(0);
                ssi_enr.set_ssi_en(false);
                ssi_enr
            });

            self.mmio.ctrlr0_mut().update(|ctrlr0| {
                ctrlr0.set_srl(enable_loopback);
            });

            self.mmio.ssi_enr_mut().write({
                let mut ssi_enr = registers::SsiEnr::from_raw(0);
                ssi_enr.set_ssi_en(true);
                ssi_enr
            });
        }

        self.set_cs_state(chip_select, true).await?;

        // TODO(https://fxbug.dev/529838127): Support DMA transfers for larger sizes.
        // This is a placeholder indicating where DMA support would be added.
        // For now, we only implement PIO.

        // A target must be selected before the transfer can begin.
        self.mmio.ser_mut().write({
            let mut ser = registers::Ser::from_raw(0);
            ser.set_ser(1);
            ser
        });

        let rxdata = self.exchange_pio_loop(txdata, rx, size);

        self.mmio.ser_mut().write(registers::Ser::from_raw(0));

        self.set_cs_state(chip_select, false).await?;

        rxdata
    }

    fn get_vmos(&self, chip_select: u32) -> Result<&HashMap<u32, RegisteredVmo>, Status> {
        match chip_select {
            0 => Ok(&self.registered_vmos),
            fidl_next_fuchsia_hardware_spiimpl::LOOPBACK_CHIP_SELECT => {
                Ok(&self.loopback_registered_vmos)
            }
            _ => Err(Status::NOT_FOUND),
        }
    }

    fn get_vmos_mut(
        &mut self,
        chip_select: u32,
    ) -> Result<&mut HashMap<u32, RegisteredVmo>, Status> {
        match chip_select {
            0 => Ok(&mut self.registered_vmos),
            fidl_next_fuchsia_hardware_spiimpl::LOOPBACK_CHIP_SELECT => {
                Ok(&mut self.loopback_registered_vmos)
            }
            _ => Err(Status::NOT_FOUND),
        }
    }

    pub fn register_vmo_impl(
        &mut self,
        chip_select: u32,
        vmo_id: u32,
        vmo: fmem::natural::Range,
        rights: SharedVmoRight,
    ) -> Result<(), Status> {
        let vmos = self.get_vmos_mut(chip_select)?;
        if vmos.contains_key(&vmo_id) {
            return Err(Status::ALREADY_EXISTS);
        }

        let vmo_size = vmo.vmo.get_size()?;
        if let Some(end) = vmo.offset.checked_add(vmo.size) {
            if end > vmo_size {
                error!("VMO range end {} is greater than VMO size {}", end, vmo_size);
                return Err(Status::INVALID_ARGS);
            }
        } else {
            error!("VMO offset {} and size {} overflow", vmo.offset, vmo.size);
            return Err(Status::INVALID_ARGS);
        }

        vmos.insert(vmo_id, RegisteredVmo { vmo, rights });
        Ok(())
    }

    pub fn unregister_vmo_impl(
        &mut self,
        chip_select: u32,
        vmo_id: u32,
    ) -> Result<zx::Vmo, Status> {
        let vmos = self.get_vmos_mut(chip_select)?;
        if let Some(registered) = vmos.remove(&vmo_id) {
            Ok(registered.vmo.vmo)
        } else {
            Err(Status::NOT_FOUND)
        }
    }

    pub fn release_registered_vmos_impl(&mut self, chip_select: u32) {
        if let Ok(vmos) = self.get_vmos_mut(chip_select) {
            vmos.clear();
        }
    }

    fn get_validated_vmo(
        &self,
        chip_select: u32,
        buffer: &fsharedmemory::natural::SharedVmoBuffer,
        required_right: SharedVmoRight,
    ) -> Result<(&zx::Vmo, u64), Status> {
        let vmos = self.get_vmos(chip_select)?;
        let registered = vmos.get(&buffer.vmo_id).ok_or(Status::NOT_FOUND)?;

        if !registered.rights.contains(required_right) {
            return Err(Status::ACCESS_DENIED);
        }

        let end = buffer.offset.checked_add(buffer.size).ok_or(Status::OUT_OF_RANGE)?;
        if end > registered.vmo.size {
            return Err(Status::OUT_OF_RANGE);
        }

        let final_offset =
            registered.vmo.offset.checked_add(buffer.offset).ok_or(Status::OUT_OF_RANGE)?;
        Ok((&registered.vmo.vmo, final_offset))
    }

    pub async fn transmit_vmo_impl(
        &mut self,
        chip_select: u32,
        buffer: &fsharedmemory::natural::SharedVmoBuffer,
    ) -> Result<(), Status> {
        // TODO(https://fxbug.dev/529838127): Use DMA instead of copying into/out of vectors.
        let mut tx_data = vec![0u8; buffer.size as usize];
        {
            let (vmo, offset) =
                self.get_validated_vmo(chip_select, buffer, SharedVmoRight::READ)?;
            vmo.read(&mut tx_data, offset)?;
        }

        self.exchange_pio(chip_select, &tx_data, false, tx_data.len()).await?;
        Ok(())
    }

    pub async fn receive_vmo_impl(
        &mut self,
        chip_select: u32,
        buffer: &fsharedmemory::natural::SharedVmoBuffer,
    ) -> Result<(), Status> {
        let rx_data = self.exchange_pio(chip_select, &[], true, buffer.size as usize).await?;

        // TODO(https://fxbug.dev/529838127): Use DMA instead of copying into/out of vectors.
        let (vmo, offset) = self.get_validated_vmo(chip_select, buffer, SharedVmoRight::WRITE)?;
        vmo.write(&rx_data, offset)?;
        Ok(())
    }

    pub async fn exchange_vmo_impl(
        &mut self,
        chip_select: u32,
        tx_buffer: &fsharedmemory::natural::SharedVmoBuffer,
        rx_buffer: &fsharedmemory::natural::SharedVmoBuffer,
    ) -> Result<(), Status> {
        if tx_buffer.size != rx_buffer.size {
            return Err(Status::INVALID_ARGS);
        }

        // TODO(https://fxbug.dev/529838127): Use DMA instead of copying into/out of vectors.
        let mut tx_data = vec![0u8; tx_buffer.size as usize];
        {
            let (tx_vmo, tx_offset) =
                self.get_validated_vmo(chip_select, tx_buffer, SharedVmoRight::READ)?;
            tx_vmo.read(&mut tx_data, tx_offset)?;
        }

        let rx_data = self.exchange_pio(chip_select, &tx_data, true, tx_data.len()).await?;

        let (rx_vmo, rx_offset) =
            self.get_validated_vmo(chip_select, rx_buffer, SharedVmoRight::WRITE)?;
        rx_vmo.write(&rx_data, rx_offset)?;
        Ok(())
    }
}

impl fidl_next_fuchsia_hardware_spiimpl::SpiImplServerHandler for DwSpiDevice {
    async fn get_chip_select_count(&mut self, responder: Responder<fspi_impl::GetChipSelectCount>) {
        let _ = responder.respond(1).await;
    }

    async fn transmit_vector(
        &mut self,
        request: Request<fspi_impl::TransmitVector>,
        responder: Responder<fspi_impl::TransmitVector>,
    ) {
        let payload = request.payload();
        let result = self
            .exchange_pio(payload.chip_select, &payload.data, false, payload.data.len())
            .await
            .map(|_| ());
        let _ = responder.respond_with(result).await;
    }

    async fn receive_vector(
        &mut self,
        request: Request<fspi_impl::ReceiveVector>,
        responder: Responder<fspi_impl::ReceiveVector>,
    ) {
        let payload = request.payload();
        let result = self
            .exchange_pio(payload.chip_select, &[], true, payload.size as usize)
            .await
            .map(|data| SpiImplReceiveVectorResponse { data });
        let _ = responder.respond_with(result).await;
    }

    async fn exchange_vector(
        &mut self,
        request: Request<fspi_impl::ExchangeVector>,
        responder: Responder<fspi_impl::ExchangeVector>,
    ) {
        let payload = request.payload();
        let result = self
            .exchange_pio(payload.chip_select, &payload.txdata, true, payload.txdata.len())
            .await
            .map(|rxdata| SpiImplExchangeVectorResponse { rxdata });
        let _ = responder.respond_with(result).await;
    }

    async fn lock_bus(
        &mut self,
        _request: Request<fspi_impl::LockBus>,
        responder: Responder<fspi_impl::LockBus>,
    ) {
        let _ = responder.respond_err(Status::NOT_SUPPORTED).await;
    }

    async fn unlock_bus(
        &mut self,
        _request: Request<fspi_impl::UnlockBus>,
        responder: Responder<fspi_impl::UnlockBus>,
    ) {
        let _ = responder.respond_err(Status::NOT_SUPPORTED).await;
    }

    async fn register_vmo(
        &mut self,
        request: Request<fspi_impl::RegisterVmo>,
        responder: Responder<fspi_impl::RegisterVmo>,
    ) {
        let payload = request.payload();
        let result = self.register_vmo_impl(
            payload.chip_select,
            payload.vmo_id,
            payload.vmo,
            payload.rights,
        );
        let _ = responder.respond_with(result).await;
    }

    async fn unregister_vmo(
        &mut self,
        request: Request<fspi_impl::UnregisterVmo>,
        responder: Responder<fspi_impl::UnregisterVmo>,
    ) {
        let payload = request.payload();
        let result = self
            .unregister_vmo_impl(payload.chip_select, payload.vmo_id)
            .map(|vmo| SpiImplUnregisterVmoResponse { vmo });
        let _ = responder.respond_with(result).await;
    }

    async fn release_registered_vmos(
        &mut self,
        request: Request<fspi_impl::ReleaseRegisteredVmos>,
    ) {
        let payload = request.payload();
        self.release_registered_vmos_impl(payload.chip_select);
    }

    async fn transmit_vmo(
        &mut self,
        request: Request<fspi_impl::TransmitVmo>,
        responder: Responder<fspi_impl::TransmitVmo>,
    ) {
        let payload = request.payload();
        let result = self.transmit_vmo_impl(payload.chip_select, &payload.buffer).await;
        let _ = responder.respond_with(result).await;
    }

    async fn receive_vmo(
        &mut self,
        request: Request<fspi_impl::ReceiveVmo>,
        responder: Responder<fspi_impl::ReceiveVmo>,
    ) {
        let payload = request.payload();
        let result = self.receive_vmo_impl(payload.chip_select, &payload.buffer).await;
        let _ = responder.respond_with(result).await;
    }

    async fn exchange_vmo(
        &mut self,
        request: Request<fspi_impl::ExchangeVmo>,
        responder: Responder<fspi_impl::ExchangeVmo>,
    ) {
        let payload = request.payload();
        let result = self
            .exchange_vmo_impl(payload.chip_select, &payload.tx_buffer, &payload.rx_buffer)
            .await;
        let _ = responder.respond_with(result).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmio::vmo::VmoMapping;
    use zx::Vmo;

    #[test]
    fn test_set_baud_rate_and_delay() {
        let vmo = Vmo::create(0x100).expect("Failed to create VMO");
        let mmio = VmoMapping::map(0, 0x100, vmo).expect("Failed to map VMO");
        let irq = zx::Interrupt::from(
            zx::VirtualInterrupt::create_virtual()
                .expect("Failed to create virtual interrupt")
                .into_handle(),
        );
        let mut device = DwSpiDevice::new(mmio, None, irq);

        device.set_baud_rate(200_000_000, 20_000_000, 25).unwrap();

        assert_eq!(device.mmio.baudr().read().sckdv(), 10);
        assert_eq!(device.mmio.rx_sample_dly().read().rsd(), 5);
    }

    #[test]
    fn test_set_baud_rate_too_slow() {
        let vmo = Vmo::create(0x100).expect("Failed to create VMO");
        let mmio = VmoMapping::map(0, 0x100, vmo).expect("Failed to map VMO");
        let irq = zx::Interrupt::from(
            zx::VirtualInterrupt::create_virtual()
                .expect("Failed to create virtual interrupt")
                .into_handle(),
        );
        let mut device = DwSpiDevice::new(mmio, None, irq);

        let result = device.set_baud_rate(200_000_000, 2_000, 0);
        assert_eq!(result.unwrap_err(), Status::INVALID_ARGS);
    }

    #[test]
    fn test_set_baud_divider_rounded_up() {
        let vmo = Vmo::create(0x100).expect("Failed to create VMO");
        let mmio = VmoMapping::map(0, 0x100, vmo).expect("Failed to map VMO");
        let irq = zx::Interrupt::from(
            zx::VirtualInterrupt::create_virtual()
                .expect("Failed to create virtual interrupt")
                .into_handle(),
        );
        let mut device = DwSpiDevice::new(mmio, None, irq);

        device.set_baud_rate(200_000_000, 3_600_000, 0).unwrap();

        assert_eq!(device.mmio.baudr().read().sckdv(), 56); // Divider rounded up to 56.
        assert_eq!(device.mmio.rx_sample_dly().read().rsd(), 0);
    }

    #[test]
    fn test_set_baud_rate_invalid_delay_remainder() {
        let vmo = Vmo::create(0x100).expect("Failed to create VMO");
        let mmio = VmoMapping::map(0, 0x100, vmo).expect("Failed to map VMO");
        let irq = zx::Interrupt::from(
            zx::VirtualInterrupt::create_virtual()
                .expect("Failed to create virtual interrupt")
                .into_handle(),
        );
        let mut device = DwSpiDevice::new(mmio, None, irq);

        let result = device.set_baud_rate(200_000_000, 20_000_000, 28);
        assert_eq!(result.unwrap_err(), Status::INVALID_ARGS);
    }

    #[test]
    fn test_set_baud_rate_invalid_delay_too_large() {
        let vmo = Vmo::create(0x100).expect("Failed to create VMO");
        let mmio = VmoMapping::map(0, 0x100, vmo).expect("Failed to map VMO");
        let irq = zx::Interrupt::from(
            zx::VirtualInterrupt::create_virtual()
                .expect("Failed to create virtual interrupt")
                .into_handle(),
        );
        let mut device = DwSpiDevice::new(mmio, None, irq);

        let result = device.set_baud_rate(200_000_000, 20_000_000, 5000);
        assert_eq!(result.unwrap_err(), Status::INVALID_ARGS);
    }
}
