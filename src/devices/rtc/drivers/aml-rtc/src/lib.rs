// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdf_component::{Driver, DriverContext, DriverError, Node, driver_register};
use fidl_fuchsia_device_fs as fdf_devfs;
use fidl_fuchsia_driver_framework as fdf_fidl;
use fidl_fuchsia_hardware_rtc as frtc;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_rtc::*;
use futures::channel::mpsc;
use futures::{StreamExt, TryStreamExt};
use log::warn;
use mmio::region::MmioRegion;
use mmio::vmo::VmoMemory;
use mmio::{register, register_block};
use pdev::{PdevExt, PlatformDevice};
use std::cell::RefCell;
use std::sync::Arc;
use zx::Status;

register! {
    #[register(offset = 0x00, mode = RW)]
    pub struct RtcCtrl(u32) {
        pub bool, osc_sel, set_osc_sel: 8;
        pub bool, enable, set_enable: 12;
    }

    #[register(offset = 0x04, mode = RW)]
    pub struct RtcCounter(u32);

    #[register(offset = 0x28, mode = RW)]
    pub struct OscinCtrl0(u32) {
        pub freq_out_select, set_freq_out_select: 29, 28;
        pub bool, clk_in_gate_en, set_clk_in_gate_en: 31;
    }

    #[register(offset = 0x2C, mode = RW)]
    pub struct OscinCtrl1(u32) {
        pub clk_div_m0, set_clk_div_m0: 11, 0;
        pub clk_div_m1, set_clk_div_m1: 23, 12;
    }

    #[register(offset = 0x34, mode = RW)]
    pub struct RtcRealTime(u32);
}

register_block! {
    struct RtcRegsBlock<M> {
        ctrl: RtcCtrl,
        counter: RtcCounter,
        oscin_ctrl0: OscinCtrl0,
        oscin_ctrl1: OscinCtrl1,
        real_time: RtcRealTime,
    }
}

struct Registers {
    regs: RtcRegsBlock<MmioRegion<VmoMemory>>,
}

impl Registers {
    fn get_rtc(&mut self) -> frtc::Time {
        let raw_seconds = self.regs.real_time().read().value();
        seconds_to_rtc(raw_seconds as u64)
    }

    fn set_rtc(&mut self, rtc: frtc::Time) -> Result<(), Status> {
        if !is_rtc_valid(&rtc) {
            return Err(Status::OUT_OF_RANGE);
        }
        let counter = RtcCounter(seconds_since_epoch(&rtc) as u32);
        self.regs.counter_mut().write(counter);
        Ok(())
    }
}

struct AmlRtcDriver {
    _node: Node,
    _scope: Arc<fasync::Scope>,
}

driver_register!(AmlRtcDriver);

impl Driver for AmlRtcDriver {
    const NAME: &str = "aml-rtc";

    async fn start(mut context: DriverContext) -> Result<Self, DriverError> {
        let node = context.take_node()?;

        let pdev = context.connect_to_pdev()?;

        let mmio = pdev.map_mmio_by_id(0).await?;
        let regs = RtcRegsBlock::new(mmio);
        let mut registers = Registers { regs };

        // Specific initialization for AML RTC
        {
            registers.regs.ctrl_mut().update(|r| r.set_osc_sel(true));

            /* Set RTC osillator to freq_out to freq_in/((N0*M0+N1*M1)/(M0+M1)) */
            /* N0 is set to 733, N1 is set to 732 by default */
            /* Enable clock_in gate of osillator 24MHz */
            registers.regs.oscin_ctrl0_mut().update(|r| {
                r.set_freq_out_select(1);
                r.set_clk_in_gate_en(true);
            });

            /* Set M0 to 2, M1 to 3, so freq_out = 32768 Hz */
            registers.regs.oscin_ctrl1_mut().update(|r| {
                r.set_clk_div_m0(1);
                r.set_clk_div_m1(2);
            });

            registers.regs.ctrl_mut().update(|r| r.set_enable(true));
        }

        // Wait for RTC to work correctly (5us in C++ driver)
        fasync::Timer::new(std::time::Duration::from_micros(5)).await;

        // Initialize and sanitize RTC
        let rtc = registers.get_rtc();
        let sanitized_rtc = sanitize_rtc(rtc);
        let _ = registers.set_rtc(sanitized_rtc);

        let scope = Arc::new(fasync::Scope::new_with_name("driver"));
        let mut fs = ServiceFs::new();

        let (tx, rx) = mpsc::unbounded::<frtc::DeviceRequestStream>();
        let tx_devfs = tx.clone();

        fs.dir("svc").add_fidl_service(move |stream: frtc::DeviceRequestStream| {
            let _ = tx.unbounded_send(stream);
        });

        scope.spawn_local(async move {
            let registers = RefCell::new(registers);
            let registers_ref = &registers;

            rx.for_each_concurrent(None, |mut stream| async move {
                while let Ok(Some(request)) = stream.try_next().await {
                    match request {
                        frtc::DeviceRequest::Get { responder } => {
                            let rtc = registers_ref.borrow_mut().get_rtc();
                            let _ = responder.send(Ok(&rtc));
                        }
                        frtc::DeviceRequest::Set2 { rtc, responder } => {
                            let result = registers_ref.borrow_mut().set_rtc(rtc);
                            let _ = responder.send(result.map_err(|e| e.into_raw()));
                        }
                        _ => {
                            warn!("Unknown request");
                        }
                    }
                }
            })
            .await;
        });

        // Serve Devfs.
        let (connector_client, connector_server) =
            fidl::endpoints::create_endpoints::<fdf_devfs::ConnectorMarker>();
        let mut connector_stream = connector_server.into_stream();

        let devfs_args = fdf_fidl::DevfsAddArgs {
            connector: Some(connector_client),
            class_name: Some("rtc".to_string()),
            ..Default::default()
        };

        let node_args = fdf_fidl::NodeAddArgs {
            name: Some("aml-rtc".to_string()),
            devfs_args: Some(devfs_args),
            ..Default::default()
        };

        let _child_controller = node.add_child(node_args).await?;

        scope.spawn(async move {
            while let Ok(Some(request)) = connector_stream.try_next().await {
                match request {
                    fdf_devfs::ConnectorRequest::Connect { server, .. } => {
                        let stream = fidl::endpoints::ServerEnd::<frtc::DeviceMarker>::new(server)
                            .into_stream();
                        let _ = tx_devfs.unbounded_send(stream);
                    }
                }
            }
        });

        context.serve_outgoing(&mut fs)?;
        scope.spawn(fs.collect());

        Ok(AmlRtcDriver { _node: node, _scope: scope })
    }

    async fn stop(&self) {}
}
