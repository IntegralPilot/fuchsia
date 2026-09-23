// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdf_component::{
    Driver, DriverContext, DriverError, Node, NodeBuilder, ServiceOffer, driver_register,
};
use fidl_next::{Request, Responder, ServerEnd};
use fidl_next_fuchsia_hardware_adcimpl as adcimpl;
use fidl_next_fuchsia_hardware_adcimpl::device::{GetResolution, GetSample};
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use futures::StreamExt;
use futures::channel::mpsc;
use log::error;
use pdev::{PdevExt, PlatformDevice};
use zx::Status;

use fdf_metadata::MetadataServer;

use mmio::region::MmioRegion;
use mmio::vmo::VmoMemory;
mod registers;
use registers::*;

const MAX_CHANNELS: u8 = 8;
const SAR_ADC_RESOLUTION: u8 = 10;

const CLK_SRC_OSCIN: u32 = 0;

struct AmlSaradcDevice<K: zx::InterruptKind> {
    adc_regs: AdcRegsBlock<MmioRegion<VmoMemory>>,
    ao_regs: AoRegsBlock<MmioRegion<VmoMemory>>,
    irq: zx::Interrupt<K>,
}

struct AmlSaradc {
    _node: Node,
    tx: mpsc::UnboundedSender<AdcRequest>,
    _scope: fasync::Scope,
}

driver_register!(AmlSaradc);

impl<K: zx::InterruptKind> AmlSaradcDevice<K> {
    fn set_clock(&mut self, src: u32, div: u32) {
        self.ao_regs.ao_sar_clk_mut().update(|reg| {
            reg.set_clk_src(src);
            reg.set_clk_div(div);
        });
    }

    fn clk_ena(&mut self, ena: bool) {
        self.ao_regs.ao_sar_clk_mut().update(|reg| {
            reg.set_clk_ena(ena);
        });
    }

    async fn enable(&mut self, ena: bool) {
        if ena {
            self.adc_regs.reg11_mut().update(|reg| {
                // Enable bandgap reference
                reg.set_ts_vbg_en(true);
                // Set common mode vref
                reg.set_rsv6(false);
                // Select bandgap as reference
                reg.set_rsv5(false);
            });
            self.adc_regs.reg0_mut().update(|reg| {
                // Enable IRQ
                reg.set_fifo_irq_en(true);
            });
            self.adc_regs.reg3_mut().update(|reg| {
                // Enable the ADC
                reg.set_adc_en(true);
            });
            fuchsia_async::Timer::new(std::time::Duration::from_micros(5)).await;
            self.clk_ena(true);
        } else {
            self.adc_regs.reg0_mut().update(|reg| {
                // Disable IRQ
                reg.set_fifo_irq_en(false);
            });
            self.clk_ena(false);
            self.adc_regs.reg3_mut().update(|reg| {
                // Disable the ADC
                reg.set_adc_en(false);
            });
        }
        fuchsia_async::Timer::new(std::time::Duration::from_micros(10)).await;
    }

    fn stop(&mut self) {
        self.adc_regs.reg0_mut().update(|reg| {
            // Stop Conversion
            reg.set_sampling_stop(true);
            // Disable Sampling
            reg.set_sampling_enable(false);
        });
    }

    async fn hw_init(&mut self) {
        {
            let adc_regs = &mut self.adc_regs;
            adc_regs.reg0_mut().write({
                let mut reg = Reg0::default();
                reg.set_val(0x84004040);
                reg
            });
            adc_regs.reg0_mut().update(|reg| {
                // Set IRQ trigger to one sample.
                reg.set_fifo_cnt_irq(1);
            });

            // Set channel list to only channel zero
            adc_regs.chan_list_mut().write(ChanList(0x00000000));
            // Disable averaging modes
            adc_regs.avg_cntl_mut().write(AvgCntl(0x00000000));
            adc_regs.reg3_mut().write({
                let mut reg = Reg3::default();
                reg.set_val(0x9388000a);
                reg
            });
            adc_regs.delay_mut().write(Delay(0x010a000a));
            adc_regs.aux_sw_mut().write(AuxSw(0x03eb1a0c));
            adc_regs.chan_10_sw_mut().write(Chan10Sw(0x008c000c));
            adc_regs.detect_idle_sw_mut().write(DetectIdleSw(0x000c000c));

            // Disable ring counter (not used on g12)
            adc_regs.reg3_mut().update(|reg| {
                let mut val = reg.val();
                val |= 1 << 27;
                reg.set_val(val);
            });

            adc_regs.reg11_mut().update(|reg| {
                reg.set_rsv1(true);
            });

            adc_regs.reg13_mut().write(Reg13(0x00002000));
        }

        self.set_clock(CLK_SRC_OSCIN, 20);
        self.enable(true).await;
        fuchsia_async::Timer::new(std::time::Duration::from_micros(10)).await;
    }

    async fn get_sample(&mut self, channel: u8) -> Result<u32, Status> {
        if channel >= MAX_CHANNELS {
            return Err(Status::INVALID_ARGS);
        }

        self.clk_ena(false);
        self.set_clock(CLK_SRC_OSCIN, 160);
        self.clk_ena(true);

        {
            let adc_regs = &mut self.adc_regs;
            adc_regs.chan_list_mut().write(ChanList(channel as u32));

            adc_regs.detect_idle_sw_mut().write(DetectIdleSw(
                0x000c000c | ((channel as u32) << 23) | ((channel as u32) << 7),
            ));

            adc_regs.reg0_mut().update(|reg| {
                reg.set_sampling_enable(true);
            });

            adc_regs.reg0_mut().update(|reg| {
                reg.set_sampling_start(true);
            });
        }

        // Wait for interrupt
        let irq_clone = self.irq.duplicate_handle(zx::Rights::SAME_RIGHTS)?;
        let mut interrupt = std::pin::pin!(fasync::OnInterrupt::new(irq_clone));
        let _ = interrupt.next().await;

        let value = self.adc_regs.fifo_rd().read().value();
        let result = (value >> 2) & 0x3ff;

        self.stop();
        self.clk_ena(false);
        self.set_clock(CLK_SRC_OSCIN, 20);
        self.clk_ena(true);

        Ok(result)
    }
}

enum AdcRequest {
    GetSample { channel: u8, responder: Responder<GetSample> },
    Stop,
}

struct DeviceServer {
    tx: mpsc::UnboundedSender<AdcRequest>,
}

impl adcimpl::DeviceLocalServerHandler for DeviceServer {
    async fn get_resolution(&mut self, responder: Responder<GetResolution>) {
        responder
            .respond(SAR_ADC_RESOLUTION)
            .await
            .unwrap_or_else(|err| error!("Failed to send get_resolution response: {err:?}"));
    }

    async fn get_sample(&mut self, request: Request<GetSample>, responder: Responder<GetSample>) {
        let channel_id = request.payload().channel_id;
        if let Err(e) =
            self.tx.unbounded_send(AdcRequest::GetSample { channel: channel_id as u8, responder })
        {
            error!("Failed to send message to actor: {e:?}");
            let req = e.into_inner();
            let responder = match req {
                AdcRequest::GetSample { responder, .. } => responder,
                _ => unreachable!(),
            };
            let _ = responder.respond_err(Status::INTERNAL).await;
        }
    }
}

struct Service {
    tx: mpsc::UnboundedSender<AdcRequest>,
    scope: fasync::ScopeHandle,
}

impl adcimpl::ServiceHandler for Service {
    fn device(&self, server_end: ServerEnd<adcimpl::Device>) {
        let tx = self.tx.clone();
        self.scope.spawn_local(async move {
            let dispatcher = fidl_next::ServerDispatcher::new(server_end);
            let _ = dispatcher.run_local(DeviceServer { tx }).await;
        });
    }
}

impl Driver for AmlSaradc {
    const NAME: &str = "aml-saradc";

    async fn start(mut context: DriverContext) -> Result<Self, DriverError> {
        let node = context.take_node()?;

        let pdev = context.connect_to_pdev()?;

        let adc_mmio = pdev.map_mmio_by_id(0).await?;
        let ao_mmio = pdev.map_mmio_by_id(1).await?;

        let irq = pdev
            .get_interrupt_by_id(0, 0)
            .await?
            .map_err(|s| s.err().unwrap_or(zx::Status::INTERNAL))?
            .irq;

        let mut device = AmlSaradcDevice {
            adc_regs: AdcRegsBlock::new(adc_mmio),
            ao_regs: AoRegsBlock::new(ao_mmio),
            irq,
        };

        device.hw_init().await;

        let (tx, mut rx) = mpsc::unbounded::<AdcRequest>();

        let scope = fasync::Scope::new_with_name("driver");

        let metadata_server = MetadataServer::new("fuchsia.hardware.adcimpl.Metadata")
            .forward_from_pdev_next(&pdev)
            .await?;

        let mut outgoing = ServiceFs::new();
        let offer = ServiceOffer::<adcimpl::Service>::new_next()
            .add_default_named_next(
                &mut outgoing,
                "default",
                Service { tx: tx.clone(), scope: scope.to_handle() },
            )
            .build_driver_offer();

        let mut child_builder = NodeBuilder::new("aml-saradc").add_offer(offer);

        if let Some(offer) = metadata_server.serve(&mut outgoing, scope.to_handle(), "default") {
            child_builder = child_builder.add_offer(offer);
        }

        let child_node = child_builder.build();
        node.add_child(child_node).await?;

        context.serve_outgoing(&mut outgoing)?;
        scope.spawn(outgoing.collect());

        let mut device_for_task = device;
        scope.spawn_local(async move {
            while let Some(req) = rx.next().await {
                match req {
                    AdcRequest::GetSample { channel, responder } => {
                        let result = device_for_task.get_sample(channel).await;
                        match result {
                            Ok(value) => {
                                let _ = responder.respond(value).await;
                            }
                            Err(status) => {
                                let _ = responder.respond_err(status).await;
                            }
                        }
                    }
                    AdcRequest::Stop => {
                        device_for_task.stop();
                        device_for_task.enable(false).await;
                        break;
                    }
                }
            }
        });

        Ok(AmlSaradc { _node: node, tx, _scope: scope })
    }

    async fn stop(&self) {
        let _ = self.tx.unbounded_send(AdcRequest::Stop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmio::vmo::VmoMapping;
    use zx::Vmo;

    #[fuchsia::test]
    async fn test_get_resolution() {
        assert_eq!(SAR_ADC_RESOLUTION, 10);
    }

    #[fuchsia::test]
    async fn test_get_sample() {
        let adc_vmo = Vmo::create(1024).unwrap();
        let ao_vmo = Vmo::create(1024).unwrap();

        // Write to FIFO_RD (0x18)
        adc_vmo.write(&0x4u32.to_le_bytes(), 0x18).unwrap();

        let adc_mmio = VmoMapping::map(0, 1024, adc_vmo).unwrap();
        let ao_mmio = VmoMapping::map(0, 1024, ao_vmo).unwrap();

        let irq: zx::Interrupt<zx::VirtualInterruptKind, zx::BootTimeline> =
            zx::Interrupt::create_virtual().unwrap();
        let irq_clone = irq.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();

        let mut device = AmlSaradcDevice {
            adc_regs: AdcRegsBlock::new(adc_mmio),
            ao_regs: AoRegsBlock::new(ao_mmio),
            irq,
        };

        // Trigger interrupt in a background task
        let irq_trigger = irq_clone;
        fasync::Task::spawn(async move {
            fasync::Timer::new(std::time::Duration::from_millis(10)).await;
            irq_trigger.trigger(zx::Instant::INFINITE).unwrap();
        })
        .detach();

        let result = device.get_sample(0).await.unwrap();
        assert_eq!(result, 1); // 0x4 >> 2 = 1
    }

    #[fuchsia::test]
    async fn test_get_sample_invalid_args() {
        let irq = zx::Interrupt::create_virtual().unwrap();

        let adc_vmo = Vmo::create(1024).unwrap();
        let ao_vmo = Vmo::create(1024).unwrap();
        let adc_mmio = VmoMapping::map(0, 1024, adc_vmo).unwrap();
        let ao_mmio = VmoMapping::map(0, 1024, ao_vmo).unwrap();

        let mut device = AmlSaradcDevice {
            adc_regs: AdcRegsBlock::new(adc_mmio),
            ao_regs: AoRegsBlock::new(ao_mmio),
            irq,
        };

        let result = device.get_sample(8).await;
        assert!(result.is_err());
        assert_eq!(result.err().unwrap(), Status::INVALID_ARGS);
    }
}
