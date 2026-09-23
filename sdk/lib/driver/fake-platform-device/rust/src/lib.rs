// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![warn(missing_docs)]

//! fake_pdev provides a fake platform-device implementation that can be used in unit tests.

use fake_bti::FakeBti;
use fidl_fuchsia_driver_framework as fdf;
use fidl_next::{Request, Responder};
use fidl_next_fuchsia_hardware_platform_device::{self as fdevice, DeviceServerHandler};
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_sync::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use zx::Status;

#[derive(Default)]
/// Holds resources used to create a `FakePDev` instance.
pub struct Config {
    /// If true, a BTI will be generated lazily if it does not exist.
    pub use_fake_bti: bool,
    /// If true, an SMC will be generated lazily if it does not exist.
    pub use_fake_smc: bool,
    /// If true, an interrupt will be generated lazily if it does not exist.
    pub use_fake_irq: bool,
    /// Key is the index of the MMIO.
    pub mmios: HashMap<u32, fdevice::natural::Mmio>,
    /// Maps the name of an MMIO to the index of the MMIO. The key is the name of the MMIO and the
    /// value is the index of the MMIO.
    pub mmio_names: HashMap<String, u32>,
    /// Key is the index of the interrupt.
    pub irqs: HashMap<u32, zx::Interrupt>,
    /// Maps the name of an interrupt to the index of the interrupt. The key is the name of the
    /// interrupt and the value is the index of the interrupt.
    pub irq_names: HashMap<String, u32>,
    /// Key is the index of the BTI.
    pub btis: HashMap<u32, zx::Bti>,
    /// Maps the name of an BTI to the index of the BTI. The key is the name of the BTI and the
    /// value is the index of the BTI.
    pub bti_names: HashMap<String, u32>,
    /// Key is the index of the SMC.
    pub smcs: HashMap<u32, zx::Resource>,
    /// The info to pass provide to `GetNodeDeviceInfo()`.
    pub device_info: Option<fdevice::natural::NodeDeviceInfo>,
    /// The info to pass provide to `GetBoardInfo()`.
    pub board_info: Option<fdevice::natural::BoardInfo>,
    /// The power elements to provide to `GetPowerConfiguration()`.
    pub power_elements: Vec<fidl_next_fuchsia_hardware_power::natural::PowerElementConfiguration>,
}

struct FakePDevState {
    config: Config,
    metadata: HashMap<String, Vec<u8>>,
}

#[derive(Clone)]
/// A fake implementation of the fuchsia.hardware.platform.device.Device protocol.
pub struct FakePDev {
    state: Arc<Mutex<FakePDevState>>,
}

impl Default for FakePDev {
    fn default() -> Self {
        Self::new()
    }
}

impl FakePDev {
    /// Creates a new `FakePDev` with an empty config and metadata.
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(FakePDevState {
                config: Config::default(),
                metadata: HashMap::new(),
            })),
        }
    }

    /// Sets the config after the `FakePDev` has been created.
    pub fn set_config(&self, config: Config) {
        self.state.lock().config = config;
    }

    /// Adds the given metadata to be provided through `GetMetadata()`.
    pub fn add_metadata(&self, id: &str, data: Vec<u8>) {
        self.state.lock().metadata.insert(id.to_string(), data);
    }

    /// Serves fuchsia.hardware.platform.device.Service with the given `ServiceFs` and instance name.
    pub fn serve(
        &self,
        service_fs: &mut ServiceFs<fuchsia_component::server::ServiceObj<'static, ()>>,
        scope: fasync::ScopeHandle,
        instance_name: &str,
    ) -> fdf::Offer {
        let state_clone = self.state.clone();

        fdf_component::ServiceOffer::<fdevice::Service>::new_next()
            .add_default_named_next(
                service_fs,
                instance_name,
                FakePDevService { state: state_clone, scope },
            )
            .build_zircon_offer_next()
    }
}

struct FakePDevService {
    state: Arc<Mutex<FakePDevState>>,
    scope: fasync::ScopeHandle,
}

impl fdevice::ServiceHandler for FakePDevService {
    fn device(&self, server_end: fidl_next::ServerEnd<fdevice::Device>) {
        server_end.spawn_on(FakePDevServer { state: self.state.clone() }, &self.scope);
    }
}

struct FakePDevServer {
    state: Arc<Mutex<FakePDevState>>,
}

impl FakePDevServer {
    fn duplicate_mmio(mmio: &fdevice::natural::Mmio) -> fdevice::natural::Mmio {
        let dup_vmo =
            mmio.vmo.as_ref().map(|v| v.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap());
        fdevice::natural::Mmio {
            offset: mmio.offset,
            size: mmio.size,
            vmo: dup_vmo.map(Into::into),
        }
    }
}

impl DeviceServerHandler for FakePDevServer {
    async fn get_mmio_by_id(
        &mut self,
        request: Request<fdevice::device::GetMmioById>,
        responder: Responder<fdevice::device::GetMmioById>,
    ) {
        let index = request.payload().index;
        let mmio_clone =
            self.state.lock().config.mmios.get(&index).map(FakePDevServer::duplicate_mmio);
        if let Some(mmio) = mmio_clone {
            let _ = responder.respond(mmio).await;
        } else {
            let _ = responder.respond_err(Status::NOT_FOUND).await;
        }
    }

    async fn get_mmio_by_name(
        &mut self,
        request: Request<fdevice::device::GetMmioByName>,
        responder: Responder<fdevice::device::GetMmioByName>,
    ) {
        let name = request.payload().name.as_str().to_string();
        let mmio_clone = {
            let state = self.state.lock();
            state
                .config
                .mmio_names
                .get(&name)
                .and_then(|idx| state.config.mmios.get(idx))
                .map(FakePDevServer::duplicate_mmio)
        };

        let _ = responder.respond_with(mmio_clone.ok_or(Status::NOT_FOUND)).await;
    }

    async fn get_interrupt_by_id(
        &mut self,
        request: Request<fdevice::device::GetInterruptById>,
        responder: Responder<fdevice::device::GetInterruptById>,
    ) {
        let index = request.payload().index;
        let (irq_res, use_fake) = {
            let state = self.state.lock();
            let irq_res =
                state.config.irqs.get(&index).map(|i| i.duplicate_handle(zx::Rights::SAME_RIGHTS));
            (irq_res, state.config.use_fake_irq)
        };

        let res: Result<zx::Interrupt, zx::Status> = if let Some(res) = irq_res {
            res
        } else if use_fake {
            zx::VirtualInterrupt::create_virtual().map(|irq| zx::Interrupt::from(irq.into_handle()))
        } else {
            Err(Status::NOT_FOUND)
        };

        match res {
            Ok(irq) => {
                let _ = responder.respond(irq).await;
            }
            Err(e) => {
                let _ = responder.respond_err(e).await;
            }
        }
    }

    async fn get_interrupt_by_name(
        &mut self,
        request: Request<fdevice::device::GetInterruptByName>,
        responder: Responder<fdevice::device::GetInterruptByName>,
    ) {
        let name = request.payload().name.as_str().to_string();
        let (irq_res, use_fake) = {
            let state = self.state.lock();
            let irq_res = state
                .config
                .irq_names
                .get(&name)
                .and_then(|idx| state.config.irqs.get(idx))
                .map(|i| i.duplicate_handle(zx::Rights::SAME_RIGHTS));
            (irq_res, state.config.use_fake_irq)
        };

        let res = if let Some(res) = irq_res {
            res
        } else if use_fake {
            zx::VirtualInterrupt::create_virtual().map(|irq| zx::Interrupt::from(irq.into_handle()))
        } else {
            Err(Status::NOT_FOUND)
        };

        match res {
            Ok(irq) => {
                let _ = responder.respond(irq).await;
            }
            Err(e) => {
                let _ = responder.respond_err(e).await;
            }
        }
    }

    async fn get_bti_by_id(
        &mut self,
        request: Request<fdevice::device::GetBtiById>,
        responder: Responder<fdevice::device::GetBtiById>,
    ) {
        let index = request.payload().index;
        let (bti_res, use_fake) = {
            let state = self.state.lock();
            let bti_res =
                state.config.btis.get(&index).map(|b| b.duplicate_handle(zx::Rights::SAME_RIGHTS));
            (bti_res, state.config.use_fake_bti)
        };

        let res = if let Some(res) = bti_res {
            res
        } else if use_fake {
            FakeBti::create().and_then(|fake| fake.duplicate_handle(zx::Rights::SAME_RIGHTS))
        } else {
            Err(Status::NOT_FOUND)
        };

        match res {
            Ok(bti) => {
                let _ = responder.respond(bti).await;
            }
            Err(status) => {
                let _ = responder.respond_err(status).await;
            }
        }
    }

    async fn get_bti_by_name(
        &mut self,
        request: Request<fdevice::device::GetBtiByName>,
        responder: Responder<fdevice::device::GetBtiByName>,
    ) {
        let name = request.payload().name.as_str().to_string();
        let (bti_res, use_fake) = {
            let state = self.state.lock();
            let bti_res = state
                .config
                .bti_names
                .get(&name)
                .and_then(|idx| state.config.btis.get(idx))
                .map(|b| b.duplicate_handle(zx::Rights::SAME_RIGHTS));
            (bti_res, state.config.use_fake_bti)
        };

        let res = if let Some(res) = bti_res {
            res
        } else if use_fake {
            FakeBti::create().and_then(|fake| fake.duplicate_handle(zx::Rights::SAME_RIGHTS))
        } else {
            Err(Status::NOT_FOUND)
        };

        match res {
            Ok(bti) => {
                let _ = responder.respond(bti).await;
            }
            Err(status) => {
                let _ = responder.respond_err(status).await;
            }
        }
    }

    async fn get_smc_by_id(
        &mut self,
        request: Request<fdevice::device::GetSmcById>,
        responder: Responder<fdevice::device::GetSmcById>,
    ) {
        let index = request.payload().index;
        let smc_res = self
            .state
            .lock()
            .config
            .smcs
            .get(&index)
            .map(|s| s.duplicate_handle(zx::Rights::SAME_RIGHTS));

        let res = smc_res.unwrap_or(Err(Status::NOT_FOUND));

        match res {
            Ok(dup) => {
                let _ = responder.respond(dup).await;
            }
            Err(status) => {
                let _ = responder.respond_err(status).await;
            }
        }
    }

    async fn get_smc_by_name(
        &mut self,
        _request: Request<fdevice::device::GetSmcByName>,
        responder: Responder<fdevice::device::GetSmcByName>,
    ) {
        let _ = responder.respond_err(Status::NOT_FOUND).await;
    }

    async fn get_power_configuration(
        &mut self,
        responder: Responder<fdevice::device::GetPowerConfiguration>,
    ) {
        let power_elements = self.state.lock().config.power_elements.clone();
        let _ = responder.respond(power_elements).await;
    }

    async fn get_node_device_info(
        &mut self,
        responder: Responder<fdevice::device::GetNodeDeviceInfo>,
    ) {
        let device_info = self.state.lock().config.device_info.clone();
        let _ = responder.respond_with(device_info.ok_or(Status::NOT_SUPPORTED)).await;
    }

    async fn get_board_info(&mut self, responder: Responder<fdevice::device::GetBoardInfo>) {
        let board_info = self.state.lock().config.board_info.clone();
        let _ = responder.respond_with(board_info.ok_or(Status::NOT_SUPPORTED)).await;
    }

    async fn get_metadata(
        &mut self,
        request: Request<fdevice::device::GetMetadata>,
        responder: Responder<fdevice::device::GetMetadata>,
    ) {
        let metadata = self.state.lock().metadata.get(request.payload().id.as_str()).cloned();
        if let Some(data) = metadata {
            let _ = responder.respond(data).await;
        } else {
            let _ = responder.respond_err(Status::NOT_FOUND).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl_next::fuchsia::create_channel;

    async fn run_test_with_config<F, Fut>(config: Config, test_func: F)
    where
        F: FnOnce(fidl_next::Client<fdevice::Device>, FakePDev) -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        let fake_pdev = FakePDev::new();
        fake_pdev.set_config(config);

        let (client_end, server_end) = create_channel::<fdevice::Device>();
        let server = FakePDevServer { state: fake_pdev.state.clone() };
        let scope = fasync::Scope::new_with_name("test");
        server_end.spawn_on(server, &scope);

        let client = client_end.spawn();
        test_func(client, fake_pdev).await;
    }

    #[fuchsia::test]
    async fn test_get_mmios() {
        let mut mmios = HashMap::new();
        let vmo = zx::Vmo::create(11).unwrap();
        mmios.insert(
            5,
            fdevice::natural::Mmio { offset: Some(10), size: Some(11), vmo: Some(vmo.into()) },
        );
        let mut mmio_names = HashMap::new();
        mmio_names.insert("test-name".to_string(), 5);

        run_test_with_config(
            Config { mmios, mmio_names, ..Default::default() },
            |client, _| async move {
                // By ID
                let res = client.get_mmio_by_id(5).await.unwrap();
                assert!(res.is_ok());
                let mmio = res.unwrap();
                assert_eq!(mmio.offset, Some(10));
                assert_eq!(mmio.size, Some(11));

                let res_err = client.get_mmio_by_id(4).await.unwrap();
                assert!(res_err.is_err());

                // By Name
                let res_name = client.get_mmio_by_name("test-name").await.unwrap();
                assert!(res_name.is_ok());
                let mmio_name = res_name.unwrap();
                assert_eq!(mmio_name.offset, Some(10));
                assert_eq!(mmio_name.size, Some(11));

                let res_name_err = client.get_mmio_by_name("unknown-name").await.unwrap();
                assert!(res_name_err.is_err());
            },
        )
        .await;
    }

    #[fuchsia::test]
    async fn test_invalid_mmio() {
        let mut mmios = HashMap::new();
        mmios.insert(
            5,
            fdevice::natural::Mmio {
                offset: Some(10),
                size: Some(11),
                vmo: None, // Invalid mmio handle
            },
        );
        run_test_with_config(Config { mmios, ..Default::default() }, |client, _| async move {
            let res = client.get_mmio_by_id(5).await.unwrap();
            assert!(res.is_ok());
            assert!(res.unwrap().vmo.is_none());
        })
        .await;
    }

    #[fuchsia::test]
    async fn test_get_irqs() {
        let mut irqs = HashMap::new();
        let irq = zx::VirtualInterrupt::create_virtual().unwrap();
        irqs.insert(5, zx::Interrupt::from(zx::NullableHandle::from(irq.into_handle())));
        let mut irq_names = HashMap::new();
        irq_names.insert("test-name".to_string(), 5);

        run_test_with_config(
            Config { irqs, irq_names, ..Default::default() },
            |client, _| async move {
                let res = client.get_interrupt_by_id(5, 0).await.unwrap();
                assert!(res.is_ok());

                let res_err = client.get_interrupt_by_id(4, 0).await.unwrap();
                assert!(res_err.is_err());

                let res_name = client.get_interrupt_by_name("test-name", 0).await.unwrap();
                assert!(res_name.is_ok());
            },
        )
        .await;
    }

    #[fuchsia::test]
    async fn test_get_btis() {
        let mut btis = HashMap::new();
        let bti = FakeBti::create().unwrap();
        btis.insert(5, bti.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap());

        run_test_with_config(Config { btis, ..Default::default() }, |client, _| async move {
            let res = client.get_bti_by_id(5).await.unwrap();
            assert!(res.is_ok());

            let res_err = client.get_bti_by_id(4).await.unwrap();
            assert!(res_err.is_err());
        })
        .await;
    }

    #[fuchsia::test]
    async fn test_get_smc() {
        unsafe extern "C" {
            fn fake_resource_create(
                kind: zx::sys::zx_rsrc_kind_t,
                out: *mut zx::sys::zx_handle_t,
            ) -> zx::sys::zx_status_t;
        }
        let mut raw = zx::sys::ZX_HANDLE_INVALID;
        unsafe {
            assert_eq!(fake_resource_create(zx::sys::ZX_RSRC_KIND_SMC, &mut raw), zx::sys::ZX_OK);
        }
        let smc = unsafe { zx::Resource::from(zx::Handle::from_raw(raw).unwrap()) };

        let mut smcs = HashMap::new();
        smcs.insert(5, smc);

        run_test_with_config(Config { smcs, ..Default::default() }, |client, _| async move {
            let res = client.get_smc_by_id(5).await.unwrap();
            assert!(res.is_ok());

            let res_err = client.get_smc_by_id(4).await.unwrap();
            assert!(res_err.is_err());
        })
        .await;
    }

    #[fuchsia::test]
    async fn test_get_device_info() {
        let device_info = Some(fdevice::natural::NodeDeviceInfo {
            vid: Some(1),
            pid: Some(1),
            name: Some("test device".to_string()),
            ..Default::default()
        });
        run_test_with_config(
            Config { device_info, ..Default::default() },
            |client, _| async move {
                let res = client.get_node_device_info().await.unwrap();
                assert!(res.is_ok());
                let info = res.unwrap();
                assert_eq!(info.vid, Some(1));
                assert_eq!(info.pid, Some(1));
                assert_eq!(info.name, Some("test device".to_string()));
            },
        )
        .await;
    }

    #[fuchsia::test]
    async fn test_get_board_info() {
        let board_info =
            Some(fdevice::natural::BoardInfo { vid: Some(1), pid: Some(1), ..Default::default() });
        run_test_with_config(Config { board_info, ..Default::default() }, |client, _| async move {
            let res = client.get_board_info().await.unwrap();
            assert!(res.is_ok());
            let info = res.unwrap();
            assert_eq!(info.vid, Some(1));
            assert_eq!(info.pid, Some(1));
        })
        .await;
    }

    #[fuchsia::test]
    async fn test_get_power_configuration() {
        let power_elements =
            vec![fidl_next_fuchsia_hardware_power::natural::PowerElementConfiguration {
                element: Some(fidl_next_fuchsia_hardware_power::natural::PowerElement {
                    name: Some("test power element".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            }];
        run_test_with_config(
            Config { power_elements, ..Default::default() },
            |client, _| async move {
                let res = client.get_power_configuration().await.unwrap();
                assert!(res.is_ok());
                let configs = res.unwrap().config;
                assert_eq!(configs.len(), 1);
                assert_eq!(
                    configs[0].element.as_ref().unwrap().name,
                    Some("test power element".to_string())
                );
            },
        )
        .await;
    }

    #[fuchsia::test]
    async fn test_get_metadata() {
        run_test_with_config(Default::default(), |client, fake_pdev| async move {
            fake_pdev.add_metadata("test_id", vec![1, 2, 3, 4]);
            let res = client.get_metadata("test_id").await.unwrap();
            assert!(res.is_ok());
            assert_eq!(res.unwrap().metadata, vec![1, 2, 3, 4]);

            let res_err = client.get_metadata("unknown_id").await.unwrap();
            assert!(res_err.is_err());
        })
        .await;
    }
}
