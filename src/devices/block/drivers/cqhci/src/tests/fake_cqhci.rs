// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::CqhciDriver;
use crate::command_queue::{CommandQueueHost, CommandQueueResources};
use async_trait::async_trait;
use cqhci_config::Config;
use fake_bti::{FakeBti, FakeBtiPinnedVmos};
use fdf_component::ServiceOffer;
use fdf_component::testing::harness::TestHarness;
use fdf_fidl::{DriverChannel, FidlExecutor};
use fidl_next_fuchsia_hardware_cqhci::{self as cqhci, CqhciHostInfo, EmmcPartitionId};
use fidl_next_fuchsia_hardware_inlineencryption as finlineencryption;
use fidl_next_fuchsia_hardware_rpmb as rpmb;
use fidl_next_fuchsia_hardware_sdmmc::{SdmmcHostCap, SdmmcHostInfo};
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_config::Config as _;
use fuchsia_sync::Mutex;
use futures::channel::{mpsc, oneshot};
use futures::future::BoxFuture;
use futures::{FutureExt as _, SinkExt as _, Stream, StreamExt as _};
use sdmmc_spec::{
    CQHCI_CQ_CAP_OFFSET, CQHCI_CQ_CFG_OFFSET, CQHCI_CQ_CTL_OFFSET, CQHCI_CQ_IS_OFFSET,
    CQHCI_CQ_TCN_OFFSET, CQHCI_CQ_TDBR_OFFSET, CQHCI_CQ_TDLBA_OFFSET, CQHCI_CQ_TDLBAU_OFFSET,
    CQHCI_CQ_TERRI_OFFSET, CQHCI_TASK_DESCRIPTOR_LIST_DCMD_SLOT, CommandQueueTDLDirectCmdEntry,
    CommandQueueTDLEntry, CommandQueueTransferDescriptor, CqhciCqCapsRegister, CqhciCqCfgRegister,
    CqhciCqCtlRegister, CqhciCqInterruptStatusRegister, CqhciCqTaskErrorRegister,
    EXT_CSD_BARRIER_SUPPORT, EXT_CSD_BARRIER_SUPPORT_MASK, EXT_CSD_CACHE_CTRL,
    EXT_CSD_CACHE_EN_MASK, EXT_CSD_CACHE_FLUSH_POLICY, EXT_CSD_CACHE_FLUSH_POLICY_FIFO,
    EXT_CSD_PARTITION_ACCESS_MASK, EXT_CSD_PARTITION_CONFIG, MMC_BLOCK_SIZE, MmcCommand,
    SDHCI_IS_OFFSET, SdhciInterruptStatusRegister, TransferAct, TransferBytes,
};
use std::pin::{Pin, pin};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};

/// Fakes the hardware side of the Command Queue interface.
pub struct FakeCqhci {
    host: Arc<FakeCqhciHost>,
    pub scope: fasync::Scope,
    pub handles: TestHandles,
}

impl FakeCqhci {
    pub fn new(
        hook: Option<Box<dyn Fn(u8) -> BoxFuture<'static, ()> + Send + Sync>>,
    ) -> (Self, TestHarness<CqhciDriver>) {
        let (non_cq_interrupt_sink, non_cq_interrupt_receiver) = mpsc::unbounded();
        let (rpmb_request_sender, rpmb_request_receiver) = mpsc::unbounded();
        let scope = fasync::Scope::new_with_name("test");
        let mut service_fs = ServiceFs::new();
        let config =
            Config { suspend_enabled: true }.to_vmo().expect("failed to create config vmo");
        let mut harness = TestHarness::<CqhciDriver>::new().set_config(config);

        let bti = FakeBti::create().unwrap();
        // If the number of addresses here gets smaller, it might impact tests which make
        // assumptions about the maximum number of operations that can be queued to avoid
        // tripping this limit (see MAX_PIN_OPS).
        let paddrs: Vec<_> =
            (0x100_000..).step_by(2 * zx::system_get_page_size() as usize).take(4096).collect();
        bti.set_paddrs(&paddrs);
        let task_handler = FakeTaskHandler::new(bti, hook);

        let host = Arc::new(FakeCqhciHost {
            task_handler,
            non_cq_interrupt_sink: Mutex::new(Some(non_cq_interrupt_sink)),
        });
        host.register_global();
        host.task_handler.spawn(&scope);

        let handles = TestHandles {
            hardware_irq: host.task_handler.irq.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
            non_cq_interrupt_receiver,
            rpmb_request_receiver,
        };

        let offers = [ServiceOffer::<cqhci::Service>::new_next()
            .add_named_next(
                &mut service_fs,
                "default",
                Service {
                    dispatcher: FidlExecutor::from(harness.dispatcher()),
                    rpmb_request_sender,
                    state: host.task_handler.state.clone(),
                },
            )
            .build_driver_offer()];
        for offer in offers {
            harness = harness.add_offer(offer);
        }
        harness = harness.set_driver_incoming(service_fs);

        (Self { host, scope, handles }, harness)
    }

    pub fn set_sdhci_interrupt_status(&self, value: SdhciInterruptStatusRegister) {
        let mut state = self.host.task_handler.state.lock();
        state.store32(MmioRegionType::Sdhci, SDHCI_IS_OFFSET as usize, value.raw());
        state.wake_event.notify(usize::MAX);
    }

    pub fn fail_next(&self, count: u32) {
        self.host.task_handler.fail_next(count);
    }

    pub fn fail_next_crypto_gce(&self, count: u32) {
        self.host.task_handler.fail_next_crypto_gce(count);
    }

    /// Returns a mask of the in-progress tasks.
    pub fn in_progress_tasks(&self) -> u32 {
        self.host.task_handler.state.lock().tasks_in_progress
    }

    /// Returns the number of completed tasks.
    pub fn completed_tasks(&self) -> u32 {
        self.host.task_handler.completed_tasks.load(Ordering::Relaxed)
    }
}

struct MockRpmbServer {
    /// A stream which receives a new channel end each time an RPMB request is made.
    ///
    /// The request will hang until the test sends a message on the channel.
    request_sender: mpsc::UnboundedSender<futures::channel::oneshot::Sender<()>>,
}

impl MockRpmbServer {
    fn new(request_sender: mpsc::UnboundedSender<futures::channel::oneshot::Sender<()>>) -> Self {
        Self { request_sender }
    }
}

impl rpmb::DriverRpmbServerHandler for MockRpmbServer {
    async fn get_device_info(
        &mut self,
        responder: fidl_next::Responder<rpmb::driver_rpmb::GetDeviceInfo, DriverChannel>,
    ) {
        responder
            .respond(rpmb::natural::DeviceInfo::EmmcInfo(rpmb::natural::EmmcDeviceInfo {
                cid: [0; 16],
                rpmb_size: 0,
                reliable_write_sector_count: 0,
            }))
            .await
            .ok();
    }

    async fn request(
        &mut self,
        _request: fidl_next::Request<rpmb::driver_rpmb::Request, DriverChannel>,
        responder: fidl_next::Responder<rpmb::driver_rpmb::Request, DriverChannel>,
    ) {
        let (tx, rx) = futures::channel::oneshot::channel();
        let _ = self.request_sender.unbounded_send(tx);
        let _ = rx.await;
        responder.respond(()).await.ok();
    }
}

struct MockInlineEncryptionServer {
    state: Arc<Mutex<FakeHardwareState>>,
}

impl MockInlineEncryptionServer {
    fn new(state: Arc<Mutex<FakeHardwareState>>) -> Self {
        Self { state }
    }
}

impl finlineencryption::DriverDeviceServerHandler for MockInlineEncryptionServer {
    async fn program_key(
        &mut self,
        _request: fidl_next::Request<finlineencryption::driver_device::ProgramKey, DriverChannel>,
        responder: fidl_next::Responder<
            finlineencryption::driver_device::ProgramKey,
            DriverChannel,
        >,
    ) {
        let slot = {
            let mut state = self.state.lock();
            let slot =
                (0..=255).find(|i| !state.valid_crypto_slots.contains(i)).expect("no free slots");
            state.valid_crypto_slots.insert(slot);
            slot
        };
        responder.respond(slot).await.ok();
    }

    async fn derive_raw_secret(
        &mut self,
        _request: fidl_next::Request<
            finlineencryption::driver_device::DeriveRawSecret,
            DriverChannel,
        >,
        responder: fidl_next::Responder<
            finlineencryption::driver_device::DeriveRawSecret,
            DriverChannel,
        >,
    ) {
        responder.respond(&[1u8, 2, 3, 4][..]).await.ok();
    }
}

pub struct FakeCqhciHost {
    task_handler: Arc<FakeTaskHandler>,
    /// A channel which receives a stream of all interrupts that the CQHCI driver delegates.
    /// Consumed when [`CommandQueueHost::initialize`] is called.
    /// The receiver end is made available to tests via [`TestHandles`].
    pub non_cq_interrupt_sink: Mutex<Option<mpsc::UnboundedSender<Arc<zx::VirtualInterrupt>>>>,
}

static INSTANCE: OnceLock<Arc<FakeCqhciHost>> = OnceLock::new();

impl FakeCqhciHost {
    fn global() -> Arc<Self> {
        INSTANCE.get().unwrap().clone()
    }

    fn register_global(self: &Arc<Self>) {
        INSTANCE.set(self.clone()).map_err(|_| "Already registered").unwrap();
    }

    fn resources(&self) -> CommandQueueResources {
        CommandQueueResources {
            cqhci_mmio: Box::new(FakeMmio {
                state: self.task_handler.state.clone(),
                region_type: MmioRegionType::Cqhci,
            }),
            sdhci_mmio: Box::new(FakeMmio {
                state: self.task_handler.state.clone(),
                region_type: MmioRegionType::Sdhci,
            }),
            bti: zx::Bti::from(
                self.task_handler.bti.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
            ),
            interrupt: self
                .task_handler
                .irq
                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                .unwrap()
                .into_handle()
                .into(),
        }
    }
}

/// A test implementation of [`CommandQueueHost`].
pub struct TestCommandQueueHost(Arc<FakeCqhciHost>);

impl TestCommandQueueHost {
    pub fn global() -> Box<dyn CommandQueueHost> {
        Box::new(TestCommandQueueHost(FakeCqhciHost::global())) as Box<dyn CommandQueueHost>
    }
}

#[async_trait]
impl CommandQueueHost for TestCommandQueueHost {
    async fn info(&self) -> Result<CqhciHostInfo, zx::Status> {
        let mut ext_csd = vec![0; 512];
        // Advertise all features, which make the driver take more interesting paths.
        ext_csd[EXT_CSD_CACHE_CTRL] = EXT_CSD_CACHE_EN_MASK;
        ext_csd[EXT_CSD_BARRIER_SUPPORT] = EXT_CSD_BARRIER_SUPPORT_MASK;
        ext_csd[EXT_CSD_CACHE_CTRL] = EXT_CSD_CACHE_EN_MASK;
        ext_csd[EXT_CSD_CACHE_FLUSH_POLICY] = EXT_CSD_CACHE_FLUSH_POLICY_FIFO;
        Ok(CqhciHostInfo {
            sdmmc_host_info: SdmmcHostInfo {
                max_transfer_size: u32::MAX,
                caps: SdmmcHostCap::empty(),
                max_buffer_regions: 128,
            },
            partitions: vec![
                cqhci::EmmcPartition {
                    id: EmmcPartitionId::UserDataPartition,
                    block_count: 1024,
                    block_size: 512,
                },
                cqhci::EmmcPartition {
                    id: EmmcPartitionId::BootPartition1,
                    block_count: 1024,
                    block_size: 512,
                },
            ],
            rca: 1,
            ext_csd,
        })
    }

    async fn initialize(
        &self,
        virtual_interrupt: zx::VirtualInterrupt,
        virtual_irq_lifeline: zx::EventPair,
    ) -> Result<CommandQueueResources, zx::Status> {
        let virtual_interrupt = Arc::new(virtual_interrupt);
        let non_cq_interrupt_sink = self.0.non_cq_interrupt_sink.lock().take().unwrap();
        std::thread::Builder::new()
            .name("test-irq-delegator".to_owned())
            .spawn(move || {
                let port = zx::Port::create_with_opts(zx::PortOptions::BIND_TO_INTERRUPT);
                let mut exec = fasync::LocalExecutorBuilder::new().port(port).build();
                exec.run_singlethreaded(async move {
                    let lifeline = virtual_irq_lifeline;
                    let mut virtual_irq_stream = pin!(fasync::OnInterrupt::new(
                        virtual_interrupt.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                    ));
                    let mut lifeline_waiter = pin!(
                        fasync::OnSignals::new(&lifeline, zx::Signals::EVENTPAIR_PEER_CLOSED).fuse());

                    loop {
                        futures::select! {
                            irq = virtual_irq_stream.next().fuse() => {
                                match irq.expect("Stream ended unexpectedly") {
                                    Ok(_) => {}
                                    Err(zx::Status::CANCELED) => break,
                                    Err(e) => panic!("Virtual interrupt failed: {:?}", e),
                                }
                                non_cq_interrupt_sink.unbounded_send(virtual_interrupt.clone()).unwrap();
                            }
                            _ = &mut lifeline_waiter => {
                                break;
                            }
                        }
                    }
                });
            })
            .unwrap();

        Ok(self.0.resources())
    }

    async fn enable(&self) -> Result<(), zx::Status> {
        let mut state = self.0.task_handler.state.lock();
        state.enabled = true;
        state.active_partition = EmmcPartitionId::UserDataPartition as u32;
        Ok(())
    }

    async fn disable(&self) -> Result<(), zx::Status> {
        let mut state = self.0.task_handler.state.lock();
        state.enabled = false;
        state.driver_in_recovery = false;
        Ok(())
    }
}

struct Service {
    dispatcher: FidlExecutor<fdf::AsyncDispatcher>,
    rpmb_request_sender: mpsc::UnboundedSender<futures::channel::oneshot::Sender<()>>,
    state: Arc<Mutex<FakeHardwareState>>,
}

impl cqhci::ServiceHandler for Service {
    fn cqhci(
        &self,
        _server_end: fidl_next::ServerEnd<fidl_next_fuchsia_hardware_cqhci::Cqhci, DriverChannel>,
    ) {
        unreachable!()
    }

    fn rpmb(&self, server_end: fidl_next::ServerEnd<fidl_next_fuchsia_hardware_rpmb::DriverRpmb>) {
        server_end
            .spawn_on(MockRpmbServer::new(self.rpmb_request_sender.clone()), &self.dispatcher);
    }

    fn inline_crypto(&self, server_end: fidl_next::ServerEnd<finlineencryption::DriverDevice>) {
        server_end.spawn_on(MockInlineEncryptionServer::new(self.state.clone()), &self.dispatcher);
    }
}

pub struct TestHandles {
    pub hardware_irq: zx::VirtualInterrupt,
    pub non_cq_interrupt_receiver: mpsc::UnboundedReceiver<Arc<zx::VirtualInterrupt>>,
    pub rpmb_request_receiver: mpsc::UnboundedReceiver<futures::channel::oneshot::Sender<()>>,
}

struct FakeHardwareState {
    enabled: bool,
    halt: bool,
    cq_data: Vec<u32>,
    sd_data: Vec<u32>,
    requests_to_fail: u32,
    requests_to_fail_crypto_gce: u32,
    rpmb_active: bool,
    wake_event: event_listener::Event,
    erase_group_start: Option<u32>,
    erase_group_end: Option<u32>,
    tasks_in_progress: u32,
    active_partition: u32,
    valid_crypto_slots: std::collections::HashSet<u8>,
    /// Set to true when the driver acknowledges an error interrupt, and cleared when the driver
    /// resets the hardware (by disabling CQE). While true, the driver must not attempt to submit
    /// new tasks.
    driver_in_recovery: bool,
}

impl FakeHardwareState {
    fn load32(&self, region: MmioRegionType, offset: usize) -> u32 {
        let buffer = match region {
            MmioRegionType::Cqhci => &self.cq_data,
            MmioRegionType::Sdhci => &self.sd_data,
        };
        buffer[offset / 4]
    }

    fn store32(&mut self, region: MmioRegionType, offset: usize, value: u32) {
        let buffer = match region {
            MmioRegionType::Cqhci => &mut self.cq_data,
            MmioRegionType::Sdhci => &mut self.sd_data,
        };
        buffer[offset / 4] = value;
    }
}

#[derive(Clone, Copy)]
pub enum MmioRegionType {
    Cqhci,
    Sdhci,
}

struct FakeMmio {
    state: Arc<Mutex<FakeHardwareState>>,
    region_type: MmioRegionType,
}

impl FakeMmio {
    fn buffer<'a>(&self, state: &'a FakeHardwareState) -> &'a [u32] {
        match self.region_type {
            MmioRegionType::Cqhci => &state.cq_data,
            MmioRegionType::Sdhci => &state.sd_data,
        }
    }

    fn buffer_mut<'a>(&self, state: &'a mut FakeHardwareState) -> &'a mut Vec<u32> {
        match self.region_type {
            MmioRegionType::Cqhci => &mut state.cq_data,
            MmioRegionType::Sdhci => &mut state.sd_data,
        }
    }
}

impl mmio::Mmio for FakeMmio {
    fn len(&self) -> usize {
        self.buffer(&self.state.lock()).len() * 4
    }
    fn align_offset(&self, _align: usize) -> usize {
        0
    }
    fn write_barrier(&self) {}

    fn try_load8(&self, _offset: usize) -> Result<u8, mmio::MmioError> {
        unreachable!()
    }
    fn try_load16(&self, _offset: usize) -> Result<u16, mmio::MmioError> {
        unreachable!()
    }
    fn try_load64(&self, _offset: usize) -> Result<u64, mmio::MmioError> {
        unreachable!()
    }

    fn try_load32(&self, offset: usize) -> Result<u32, mmio::MmioError> {
        let state = self.state.lock();
        let buf = self.buffer(&state);
        Ok(buf[offset / 4])
    }

    fn load8(&self, _offset: usize) -> u8 {
        unreachable!()
    }
    fn load16(&self, _offset: usize) -> u16 {
        unreachable!()
    }
    fn load64(&self, _offset: usize) -> u64 {
        unreachable!()
    }
    fn load32(&self, offset: usize) -> u32 {
        self.try_load32(offset).unwrap()
    }

    fn try_store8(&mut self, _offset: usize, _value: u8) -> Result<(), mmio::MmioError> {
        unreachable!()
    }
    fn try_store16(&mut self, _offset: usize, _value: u16) -> Result<(), mmio::MmioError> {
        unreachable!()
    }
    fn try_store64(&mut self, _offset: usize, _value: u64) -> Result<(), mmio::MmioError> {
        unreachable!()
    }

    fn try_store32(&mut self, offset: usize, value: u32) -> Result<(), mmio::MmioError> {
        let mut state = self.state.lock();
        let enabled = state.enabled;
        let driver_in_recovery = state.driver_in_recovery;
        let buf = self.buffer_mut(&mut state);

        let idx = offset / 4;
        match self.region_type {
            MmioRegionType::Cqhci => match offset {
                CQHCI_CQ_TDBR_OFFSET => {
                    assert!(enabled, "Doorbell rung while CQHCI is disabled");
                    assert!(!driver_in_recovery, "Doorbell rung while driver is in recovery");
                    // Notify the FakeTaskHandler on doorbell ring
                    let current = buf[idx];
                    let new_val = current | value;
                    buf[idx] = new_val;
                    state.wake_event.notify(usize::MAX);
                    Ok(())
                }
                CQHCI_CQ_TCN_OFFSET => {
                    // Emulate W1C semantics
                    let current = buf[idx];
                    let cleared = current & !value;
                    buf[idx] = cleared;
                    Ok(())
                }
                CQHCI_CQ_IS_OFFSET => {
                    // Emulate W1C semantics
                    let current = buf[idx];
                    let cleared = current & !value;
                    buf[idx] = cleared;
                    if CqhciCqInterruptStatusRegister::from_raw(value).is_error() {
                        state.driver_in_recovery = true;
                    }
                    Ok(())
                }
                CQHCI_CQ_CTL_OFFSET => {
                    if value & CqhciCqCtlRegister::HALT.bits() != 0 {
                        state.halt = true;
                        state.wake_event.notify(usize::MAX);
                    } else {
                        buf[idx] = value;
                    }
                    Ok(())
                }
                CQHCI_CQ_CFG_OFFSET => {
                    if buf[idx] & CqhciCqCfgRegister::CQE_ENABLE.bits() != 0
                        && value & CqhciCqCfgRegister::CQE_ENABLE.bits() == 0
                    {
                        // As per JESD84-B51B B.4.12 the doorbell is cleared when command queuing is
                        // disabled.
                        buf[CQHCI_CQ_TDBR_OFFSET / 4] = 0;
                    }
                    buf[idx] = value;
                    Ok(())
                }
                _ => {
                    buf[idx] = value;
                    Ok(())
                }
            },
            MmioRegionType::Sdhci => match offset {
                SDHCI_IS_OFFSET => {
                    // Emulate W1C semantics
                    let current = buf[idx];
                    let cleared = current & !value;
                    buf[idx] = cleared;
                    Ok(())
                }
                _ => {
                    buf[idx] = value;
                    Ok(())
                }
            },
        }
    }

    fn store8(&mut self, _offset: usize, _value: u8) {
        unreachable!()
    }
    fn store16(&mut self, _offset: usize, _value: u16) {
        unreachable!()
    }
    fn store64(&mut self, _offset: usize, _value: u64) {
        unreachable!()
    }
    fn store32(&mut self, offset: usize, value: u32) {
        self.try_store32(offset, value).unwrap();
    }
}

fn scramble_data(data: &mut [u8], slot: u8, dun: u32) {
    let key = ((slot as u64) << 32) | (dun as u64);
    let key_bytes = key.to_ne_bytes();
    for (i, byte) in data.iter_mut().enumerate() {
        *byte ^= key_bytes[i % 8];
    }
}

fn transfer_data(
    is_read: bool,
    target_vmo: &zx::Vmo,
    current_offset: u64,
    pinned: &FakeBtiPinnedVmos,
    addr: usize,
    len: usize,
    ce: bool,
    slot: u8,
    dun: u32,
) {
    if is_read {
        let mut temp_buf = target_vmo.read_to_vec(current_offset, len as u64).unwrap();
        if ce {
            scramble_data(&mut temp_buf, slot, dun);
        }
        pinned.write_bytes(addr, &temp_buf).unwrap();
    } else {
        let mut temp_buf = pinned.read_bytes(addr, len).unwrap();
        if ce {
            scramble_data(&mut temp_buf, slot, dun);
        }
        target_vmo.write(&temp_buf, current_offset).unwrap();
    }
}

struct FakeTaskHandler {
    irq: zx::VirtualInterrupt,
    state: Arc<Mutex<FakeHardwareState>>,
    bti: FakeBti,
    partition_vmos: std::collections::HashMap<u32, zx::Vmo>,
    on_task_started: Option<Box<dyn Fn(u8) -> BoxFuture<'static, ()> + Send + Sync>>,
    completed_tasks: AtomicU32,
    tasks: Mutex<Vec<fasync::Task<()>>>,
}

impl FakeTaskHandler {
    fn new(
        bti: FakeBti,
        on_task_started: Option<Box<dyn Fn(u8) -> BoxFuture<'static, ()> + Send + Sync>>,
    ) -> Arc<Self> {
        let mut partition_vmos = std::collections::HashMap::new();
        partition_vmos.insert(
            EmmcPartitionId::UserDataPartition as u32,
            zx::Vmo::create(16 * 1024 * 1024).unwrap(),
        );
        partition_vmos
            .insert(EmmcPartitionId::BootPartition1 as u32, zx::Vmo::create(1024 * 1024).unwrap());
        partition_vmos
            .insert(EmmcPartitionId::BootPartition2 as u32, zx::Vmo::create(1024 * 1024).unwrap());

        Arc::new(Self {
            irq: zx::Interrupt::create_virtual().unwrap(),
            state: Arc::new(Mutex::new(FakeHardwareState {
                enabled: false,
                halt: false,
                cq_data: {
                    let mut v = vec![0; 0x400];
                    let mut caps = CqhciCqCapsRegister(0);
                    caps.set_crypto_support(true);
                    v[CQHCI_CQ_CAP_OFFSET / 4] = caps.0;
                    v
                },
                sd_data: vec![0; 0x400],
                requests_to_fail: 0,
                requests_to_fail_crypto_gce: 0,
                rpmb_active: false,
                wake_event: event_listener::Event::new(),
                erase_group_start: None,
                erase_group_end: None,
                tasks_in_progress: 0,
                active_partition: EmmcPartitionId::UserDataPartition as u32,
                valid_crypto_slots: std::collections::HashSet::new(),
                driver_in_recovery: false,
            })),
            bti,
            partition_vmos,
            on_task_started,
            completed_tasks: AtomicU32::new(0),
            tasks: Mutex::new(Vec::new()),
        })
    }

    fn spawn(self: &Arc<Self>, scope: &fasync::Scope) -> fasync::JoinHandle<()> {
        let this = self.clone();
        scope.spawn(async move {
            loop {
                let listener = this.state.lock().wake_event.listen();
                this.poll().await;
                listener.await;
            }
        })
    }

    fn fail_next(&self, count: u32) {
        self.state.lock().requests_to_fail = count;
    }

    fn fail_next_crypto_gce(&self, count: u32) {
        self.state.lock().requests_to_fail_crypto_gce = count;
    }

    async fn poll(self: &Arc<Self>) {
        loop {
            let (to_spawn, tdl_phys) = {
                let mut state = self.state.lock();
                if state.halt {
                    if state.tasks_in_progress == 0 {
                        state.store32(
                            MmioRegionType::Cqhci,
                            CQHCI_CQ_CTL_OFFSET,
                            CqhciCqCtlRegister::HALT.bits(),
                        );
                        state.halt = false;
                        return;
                    }
                }
                // Return if HALT is set, or Command Queuing is disabled.
                if state.load32(MmioRegionType::Cqhci, CQHCI_CQ_CTL_OFFSET)
                    & CqhciCqCtlRegister::HALT.bits()
                    != 0
                    || state.load32(MmioRegionType::Cqhci, CQHCI_CQ_CFG_OFFSET)
                        & CqhciCqCfgRegister::CQE_ENABLE.bits()
                        == 0
                {
                    return;
                }
                let doorbell = state.load32(MmioRegionType::Cqhci, CQHCI_CQ_TDBR_OFFSET as usize);
                let to_spawn = doorbell & !state.tasks_in_progress;
                if to_spawn == 0 {
                    return;
                }
                state.tasks_in_progress |= to_spawn;
                let tdlba = state.load32(MmioRegionType::Cqhci, CQHCI_CQ_TDLBA_OFFSET as usize);
                let tdlbau = state.load32(MmioRegionType::Cqhci, CQHCI_CQ_TDLBAU_OFFSET as usize);
                let tdl_phys = (tdlbau as usize) << 32 | tdlba as usize;
                (to_spawn, tdl_phys)
            };

            let pinned = Arc::new(self.bti.get_pinned_vmo());
            for task_id in 0..32 {
                if (to_spawn & (1 << task_id)) != 0 {
                    let this = self.clone();
                    let pinned = pinned.clone();
                    let task = fasync::Task::spawn(async move {
                        this.process_task(task_id as u8, tdl_phys, pinned).await;
                    });
                    self.tasks.lock().push(task);
                }
            }
        }
    }

    async fn process_task(
        self: Arc<Self>,
        task_id: u8,
        tdl_phys: usize,
        pinned: Arc<FakeBtiPinnedVmos>,
    ) {
        if let Some(f) = &self.on_task_started {
            f(task_id).await;
        }

        let entry_offset =
            tdl_phys + (task_id as usize * std::mem::size_of::<CommandQueueTDLEntry>());

        let mut has_error = false;
        let mut error_is_icce = false;

        if task_id == CQHCI_TASK_DESCRIPTOR_LIST_DCMD_SLOT {
            let entry: CommandQueueTDLDirectCmdEntry = pinned.read(entry_offset).unwrap();

            let command = entry.task.cmd_index().unwrap();
            match command {
                MmcCommand::Switch => {
                    let arg = entry.task.cmd_arg();
                    let access = arg >> 24;
                    if access == 3 {
                        // WRITE
                        let index = (arg >> 16) & 0xFF;
                        let value = (arg >> 8) & 0xFF;
                        if index == EXT_CSD_PARTITION_CONFIG as u32 {
                            let switch_partition = value & !(EXT_CSD_PARTITION_ACCESS_MASK as u32);
                            self.state.lock().active_partition = switch_partition as u32;
                        }
                    } else {
                        panic!("Unexpected access for SWITCH command: {access}");
                    }
                }
                MmcCommand::SendStatus => {
                    // We leave CQHCI_CQ_CRDCT_OFFSET untouched with 0, which will get interpreted
                    // as we want.
                }
                MmcCommand::EraseGroupStart => {
                    self.state.lock().erase_group_start = Some(entry.task.cmd_arg());
                }
                MmcCommand::EraseGroupEnd => {
                    self.state.lock().erase_group_end = Some(entry.task.cmd_arg());
                }
                MmcCommand::Erase => {
                    let mut state = self.state.lock();
                    if let (Some(start), Some(end)) =
                        (state.erase_group_start.take(), state.erase_group_end.take())
                    {
                        let active_partition = state.active_partition;
                        drop(state);
                        if let Some(vmo) = self.partition_vmos.get(&active_partition)
                            && end >= start
                        {
                            let start_offset = start as u64 * MMC_BLOCK_SIZE;
                            let len = ((end - start + 1) as u64) * MMC_BLOCK_SIZE;
                            let ones = vec![0xFF; len as usize];
                            vmo.write(&ones, start_offset).unwrap();
                        }
                    } else {
                        panic!("Erase command without erase group start/end set!");
                    }
                }
                MmcCommand::QueuedTaskAddress
                | MmcCommand::QueuedTaskParams
                | MmcCommand::CommandQueueTaskManagement => {
                    // We don't do anything with these.
                }
            }
        } else {
            let entry: CommandQueueTDLEntry = pinned.read(entry_offset).unwrap();

            let block_offset = entry.task.block_offset();
            let is_read = entry.task.data_direction();
            let ce = entry.task.ce();
            let slot = entry.task.cci();
            let dun = entry.task.dun();

            if ce && !self.state.lock().valid_crypto_slots.contains(&slot) {
                has_error = true;
                error_is_icce = true;
            }

            if !has_error {
                let (target_vmo, rpmb_active, _active_partition) = {
                    let mut state = self.state.lock();
                    if !state.enabled {
                        state.tasks_in_progress &= !(1 << task_id);
                        return;
                    }
                    (
                        self.partition_vmos
                            .get(&state.active_partition)
                            .and_then(|v| v.duplicate_handle(zx::Rights::SAME_RIGHTS).ok()),
                        state.rpmb_active,
                        state.active_partition,
                    )
                };
                assert!(!rpmb_active, "task submitted while RPMB request is active!");

                if let Some(target_vmo) = target_vmo {
                    let mut current_offset = block_offset as u64 * MMC_BLOCK_SIZE;
                    let desc = entry.transfer;

                    loop {
                        if !desc.valid() {
                            break;
                        }
                        let address = desc.address() as usize;

                        if desc.act() == Ok(TransferAct::Link) {
                            let mut link_phys = address;
                            loop {
                                let link_desc: CommandQueueTransferDescriptor =
                                    pinned.read(link_phys).unwrap();

                                if !link_desc.valid() {
                                    break;
                                }

                                if link_desc.act() == Ok(TransferAct::Tran) {
                                    let mut len = link_desc.length() as usize;
                                    if len == 0 {
                                        len = TransferBytes::MAX_BYTES;
                                    }
                                    let link_addr = link_desc.address() as usize;

                                    transfer_data(
                                        is_read,
                                        &target_vmo,
                                        current_offset,
                                        &pinned,
                                        link_addr,
                                        len,
                                        ce,
                                        slot,
                                        dun,
                                    );
                                    current_offset += len as u64;
                                }
                                if link_desc.end() {
                                    break;
                                }
                                link_phys += 16;
                            }
                            break;
                        } else if desc.act() == Ok(TransferAct::Tran) {
                            let mut len = desc.length() as usize;
                            if len == 0 {
                                len = TransferBytes::MAX_BYTES;
                            }

                            transfer_data(
                                is_read,
                                &target_vmo,
                                current_offset,
                                &pinned,
                                address,
                                len,
                                ce,
                                slot,
                                dun,
                            );
                            current_offset += len as u64;
                            if desc.end() {
                                break;
                            }
                        } else {
                            panic!("Unexpected descriptor {desc:?}");
                        }
                    }
                }
            }
        }

        {
            let mut state = self.state.lock();
            let current_cq_is = state.load32(MmioRegionType::Cqhci, CQHCI_CQ_IS_OFFSET as usize);
            let mut cqhci_irq_status = CqhciCqInterruptStatusRegister::from_raw(current_cq_is);

            if state.requests_to_fail > 0 {
                state.requests_to_fail -= 1;
                cqhci_irq_status.set_response_error_detected(true);
                let mut terri = CqhciCqTaskErrorRegister(0);
                terri.set_response_mode_error_task_id(task_id);
                terri.set_response_mode_error_fields_valid(true);
                state.store32(MmioRegionType::Cqhci, CQHCI_CQ_TERRI_OFFSET as usize, terri.0);
            } else if error_is_icce {
                cqhci_irq_status.set_invalid_crypto_config_error(true);
                let mut terri = CqhciCqTaskErrorRegister(0);
                terri.set_response_mode_error_task_id(task_id);
                terri.set_response_mode_error_fields_valid(true);
                state.store32(MmioRegionType::Cqhci, CQHCI_CQ_TERRI_OFFSET as usize, terri.0);
            } else if state.requests_to_fail_crypto_gce > 0 {
                state.requests_to_fail_crypto_gce -= 1;
                cqhci_irq_status.set_general_crypto_error(true);
                let mut terri = CqhciCqTaskErrorRegister(0);
                terri.set_response_mode_error_task_id(task_id);
                terri.set_response_mode_error_fields_valid(true);
                state.store32(MmioRegionType::Cqhci, CQHCI_CQ_TERRI_OFFSET as usize, terri.0);
            } else {
                cqhci_irq_status.set_task_complete(true);
                let current_tcn = state.load32(MmioRegionType::Cqhci, CQHCI_CQ_TCN_OFFSET as usize);
                let new_tcn = current_tcn | (1 << task_id);
                state.store32(MmioRegionType::Cqhci, CQHCI_CQ_TCN_OFFSET as usize, new_tcn);
            }

            state.store32(
                MmioRegionType::Cqhci,
                CQHCI_CQ_IS_OFFSET as usize,
                cqhci_irq_status.raw(),
            );

            let current_sd_is = state.load32(MmioRegionType::Sdhci, SDHCI_IS_OFFSET as usize);
            let mut sdhci_irq_status = SdhciInterruptStatusRegister::from_raw(current_sd_is);
            sdhci_irq_status.set_cqhci_interrupt(true);
            state.store32(MmioRegionType::Sdhci, SDHCI_IS_OFFSET as usize, sdhci_irq_status.raw());

            // Clear the doorbell bit now that we've signaled completion.
            let current_db = state.load32(MmioRegionType::Cqhci, CQHCI_CQ_TDBR_OFFSET as usize);
            state.store32(
                MmioRegionType::Cqhci,
                CQHCI_CQ_TDBR_OFFSET as usize,
                current_db & !(1 << task_id),
            );

            state.tasks_in_progress &= !(1 << task_id);
            self.completed_tasks.fetch_add(1, Ordering::Relaxed);
            state.wake_event.notify(usize::MAX);
        }

        self.irq.trigger(zx::BootInstant::get()).expect("failed to trigger interrupt");
    }
}

/// Blocker can be used in tests to help block requests.  The result of `Block::hook()` can be
/// passed to `FakeCqhci::new`.
pub struct Blocker {
    should_block: Arc<AtomicU32>,
    blocked_rx: mpsc::UnboundedReceiver<oneshot::Sender<()>>,
    blocked_tx: mpsc::UnboundedSender<oneshot::Sender<()>>,
}

impl Default for Blocker {
    fn default() -> Self {
        let (blocked_tx, blocked_rx) = mpsc::unbounded();
        Self { should_block: Arc::default(), blocked_tx, blocked_rx }
    }
}

impl Blocker {
    pub fn block(&self, task_id_mask: u32) {
        self.should_block.store(task_id_mask, Ordering::Relaxed);
    }

    pub fn hook(&self) -> Box<dyn Fn(u8) -> BoxFuture<'static, ()> + Send + Sync> {
        let blocked_tx = self.blocked_tx.clone();
        let should_block = self.should_block.clone();
        Box::new(move |task_id| {
            let mut blocked_tx = blocked_tx.clone();
            let should_block = should_block.clone();
            async move {
                if should_block.load(Ordering::Relaxed) & (1 << task_id) != 0 {
                    let (unblock_tx, unblock_rx) = oneshot::channel::<()>();
                    let _ = blocked_tx.send(unblock_tx).await;
                    let _ = unblock_rx.await;
                }
            }
            .boxed()
        })
    }
}

impl Stream for Blocker {
    type Item = oneshot::Sender<()>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.blocked_rx.poll_next_unpin(cx)
    }
}
