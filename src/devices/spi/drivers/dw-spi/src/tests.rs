// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::DwSpiDriver;
use super::spi_device::registers;
use fake_clock::FakeClock;
use fake_gpio::FakeGpio;
use fake_pdev::FakePDev;
use fake_powerdomain::FakePowerDomain;
use fake_reset::FakeReset;
use fdf_component::testing::harness::TestHarness;
use fdf_fidl;
use fidl::Serializable;
use fidl_fuchsia_driver_metadata as fmetadata;
use fidl_fuchsia_hardware_spi_businfo as fspi_businfo;
use fidl_next_fuchsia_hardware_gpio as fgpio;
use fidl_next_fuchsia_hardware_platform_device as fdevice;
use fidl_next_fuchsia_hardware_sharedmemory as fsharedmemory;
use fidl_next_fuchsia_hardware_sharedmemory::natural::SharedVmoRight;
use fidl_next_fuchsia_hardware_spiimpl as fspiimpl;
use fidl_next_fuchsia_mem as fmem;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use futures::future::{self, Either};
use mmio::Register;
use zx::Vmo;

#[fuchsia::test]
async fn test_init() {
    let mut service_fs = ServiceFs::new();
    let scope = fasync::Scope::new_with_name("test");

    let pdev = FakePDev::new();

    let entries = vec![fmetadata::DictionaryEntry {
        key: "dw_spi_rx_sample_delay_ns".to_string(),
        value: fmetadata::DictionaryValue::Int64(25),
    }];
    let dict = fmetadata::Dictionary { entries: Some(entries), ..Default::default() };
    let serialized_config = fidl::persist(&dict).expect("Failed to serialize config");
    pdev.add_metadata("fuchsia.driver.metadata.Dictionary", serialized_config);

    let metadata = fspi_businfo::SpiBusMetadata {
        channels: Some(vec![fspi_businfo::SpiChannel {
            cs: Some(0),
            max_frequency_hz: Some(20_000_000),
            ..Default::default()
        }]),
        bus_id: Some(0),
        ..Default::default()
    };
    let serialized_metadata = fidl::persist(&metadata).expect("Failed to serialize metadata");
    pdev.add_metadata(fspi_businfo::SpiBusMetadata::SERIALIZABLE_NAME, serialized_metadata);

    let vmo = Vmo::create(0x100).expect("Failed to create VMO");
    vmo.set_cache_policy(zx::CachePolicy::UnCachedDevice).expect("Failed to set cache policy");
    let mapping = mapped_vmo::Mapping::create_from_vmo(
        &vmo,
        0x100,
        zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE,
    )
    .expect("Failed to map VMO");

    let dup = vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("Failed to duplicate VMO");
    let mut pdev_config: fake_pdev::Config = Default::default();
    let mmio = fdevice::natural::Mmio { offset: Some(0), size: Some(0x100), vmo: Some(dup) };
    pdev_config.mmios.insert(0, mmio);
    pdev_config.use_fake_irq = true;
    pdev.set_config(pdev_config);

    let powerdomain = FakePowerDomain::new();

    let clock_bus = FakeClock::new();
    clock_bus.set_rate(200_000_000);
    let clock_regs = FakeClock::new();

    let mut reset = FakeReset::new();

    let gpio = FakeGpio::default();

    let mut harness = TestHarness::<DwSpiDriver>::new()
        .add_offer(pdev.serve(&mut service_fs, scope.to_handle(), "pdev"))
        .add_offer(powerdomain.serve(&mut service_fs, scope.to_handle(), "power-domain"))
        .add_offer(clock_bus.serve(&mut service_fs, scope.to_handle(), "bus"))
        .add_offer(clock_regs.serve(&mut service_fs, scope.to_handle(), "registers"))
        .add_offer(reset.serve(&mut service_fs, scope.to_handle(), "reset"))
        .add_offer(gpio.serve(&mut service_fs, scope.to_handle(), "cs-0"))
        .set_driver_incoming(service_fs);

    let started_driver = harness.start_driver().await.expect("Failed to start driver");
    started_driver.stop_driver().await;

    assert!(powerdomain.enabled());
    assert!(clock_bus.enabled());
    assert!(clock_regs.enabled());
    assert!(reset.take_toggled());

    let read_u32 = |offset: usize| -> u32 {
        let mut bytes = [0u8; 4];
        mapping.read_at(offset, &mut bytes);
        u32::from_le_bytes(bytes)
    };

    let ctrlr0 = read_u32(0x0);
    assert_eq!(ctrlr0, 7);

    let ssi_enr = read_u32(0x8);
    assert_eq!(ssi_enr, 1);

    let baudr = read_u32(0x14);
    assert_eq!(baudr, 10);

    let rx_sample_dly = read_u32(0xf0);
    assert_eq!(rx_sample_dly, 5);
}

#[fuchsia::test]
async fn test_exchange_vector() {
    let mut service_fs = ServiceFs::new();
    let scope = fasync::Scope::new_with_name("test");

    let pdev = FakePDev::new();

    let vmo = Vmo::create(0x100).expect("Failed to create VMO");
    vmo.set_cache_policy(zx::CachePolicy::UnCachedDevice).expect("Failed to set cache policy");
    let mapping = mapped_vmo::Mapping::create_from_vmo(
        &vmo,
        0x100,
        zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE,
    )
    .expect("Failed to map VMO");

    let dup = vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("Failed to duplicate VMO");
    let irq = zx::VirtualInterrupt::create_virtual().expect("Failed to create virtual interrupt");
    let irq_dup =
        irq.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("Failed to duplicate interrupt");

    let mut pdev_config: fake_pdev::Config = Default::default();
    let mmio = fdevice::natural::Mmio { offset: Some(0), size: Some(0x100), vmo: Some(dup) };
    pdev_config.mmios.insert(0, mmio);
    pdev_config.irqs.insert(0, zx::Interrupt::from(irq.into_handle()));
    pdev.set_config(pdev_config);

    let powerdomain = FakePowerDomain::new();

    let clock_bus = FakeClock::new();
    let clock_regs = FakeClock::new();

    let reset = FakeReset::new();

    let gpio = FakeGpio::default();

    let mut harness = TestHarness::<DwSpiDriver>::new()
        .add_offer(pdev.serve(&mut service_fs, scope.to_handle(), "pdev"))
        .add_offer(powerdomain.serve(&mut service_fs, scope.to_handle(), "power-domain"))
        .add_offer(clock_bus.serve(&mut service_fs, scope.to_handle(), "bus"))
        .add_offer(clock_regs.serve(&mut service_fs, scope.to_handle(), "registers"))
        .add_offer(reset.serve(&mut service_fs, scope.to_handle(), "reset"))
        .add_offer(gpio.serve(&mut service_fs, scope.to_handle(), "cs-0"))
        .set_driver_incoming(service_fs);

    let dispatcher = fdf_fidl::FidlExecutor::from(harness.dispatcher().clone());
    let started_driver = harness.start_driver().await.expect("Failed to start driver");

    let spi_service: fdf_component::ServiceInstance<fspiimpl::Service> =
        started_driver.driver_outgoing().service().connect_next().unwrap();

    let (client_end, server_end) = fdf_fidl::create_channel();
    spi_service.device(server_end).unwrap();
    let client = client_end.spawn_on(&dispatcher);

    let read_u32 = |offset: usize| -> u32 {
        let mut bytes = [0u8; 4];
        mapping.read_at(offset, &mut bytes);
        u32::from_le_bytes(bytes)
    };

    let write_u32 = |offset: usize, value: u32| {
        mapping.write_at(offset, &value.to_le_bytes());
    };

    // Set TXFLR and ISR.TXEIS to indicate TX FIFO empty, then trigger the interrupt.
    write_u32(0x20, 0x0);
    write_u32(0x30, 0x01);

    irq_dup.trigger(zx::BootInstant::ZERO).expect("Failed to trigger interrupt");

    // Call exchange_vector(), but don't await on it yet.
    let txdata = vec![1u8, 2, 3, 4, 5];
    let exchange_future = client.exchange_vector(0, &txdata);
    let exchange_future = std::pin::pin!(exchange_future);

    // The driver should handle the interrupt then ack it. Wait for this to happen before starting
    // the RX portion of the transfer.
    let untriggered_future =
        fasync::OnSignals::new(&irq_dup, zx::Signals::VIRTUAL_INTERRUPT_UNTRIGGERED);
    let untriggered_future = std::pin::pin!(untriggered_future);

    // The actual exchange_vector() FIDL call is made the first time its future is polled, so the
    // two futures must be selected here. The untriggered future will always complete first.
    let exchange_future = match future::select(exchange_future, untriggered_future).await {
        Either::Left((res, _)) => {
            panic!("exchange_vector completed prematurely: {:?}", res);
        }
        Either::Right((res, remaining_exchange_future)) => {
            res.expect("Failed to wait for untriggered signal");
            remaining_exchange_future
        }
    };

    // Verify register values after the first interrupt (TX empty) is handled:
    // - TXFTLR: Remains at the default 128.
    // - RXFTLR: Set to 4 (calculated as rx_remaining (5) - 1).
    // - IMR: Only RXFIFO interrupt remains unmasked (and error interrupts). TX FIFO empty interrupt
    //        is masked since TX is done.
    assert_eq!(read_u32(0x18), 128);
    assert_eq!(read_u32(0x1c), 4);
    assert_eq!(read_u32(0x2c), 0x1e);

    // Set RXFLR and ISR.RXFIS to indicate that 5 bytes can be read from the RX FIFO, then trigger
    // the interrupt again.
    write_u32(0x24, 0x5);
    write_u32(0x30, 0x10);

    irq_dup.trigger(zx::BootInstant::ZERO).expect("Failed to trigger interrupt");

    // The driver should be able to complete the request now.
    let response = exchange_future.await;
    assert!(response.is_ok());

    // Verify register values after the transfer is complete:
    // - TXFTLR: Remains at the default 128.
    // - RXFTLR: Set to 0 (calculated as rx_remaining.max(1) - 1, which is 1 - 1 = 0 when
    //           rx_remaining is 0).
    // - IMR: All interrupts are masked.
    assert_eq!(read_u32(0x18), 128);
    assert_eq!(read_u32(0x1c), 0);
    assert_eq!(read_u32(0x2c), 0);

    let rxdata = response.unwrap().unwrap().rxdata;
    assert_eq!(rxdata.len(), txdata.len());

    // Check the last value that was written to the FIFO.
    assert_eq!(read_u32(0x60), 5);
    // The RX path should have read the last value that was written to the FIFO.
    assert_eq!(rxdata, vec![5, 5, 5, 5, 5]);

    started_driver.stop_driver().await;
}

#[fuchsia::test]
async fn test_vmo_registration() {
    let mut service_fs = ServiceFs::new();
    let scope = fasync::Scope::new_with_name("test");

    let vmo = Vmo::create(0x100).expect("Failed to create VMO");
    let mmio = fdevice::natural::Mmio { offset: Some(0), size: Some(0x100), vmo: Some(vmo) };

    let mut pdev_config: fake_pdev::Config = Default::default();
    pdev_config.mmios.insert(0, mmio);
    pdev_config.use_fake_irq = true;

    let pdev = FakePDev::new();
    pdev.set_config(pdev_config);

    let powerdomain = FakePowerDomain::new();
    let clock_bus = FakeClock::new();
    let clock_regs = FakeClock::new();
    let reset = FakeReset::new();
    let gpio = FakeGpio::default();

    let mut harness = TestHarness::<DwSpiDriver>::new()
        .add_offer(pdev.serve(&mut service_fs, scope.to_handle(), "pdev"))
        .add_offer(powerdomain.serve(&mut service_fs, scope.to_handle(), "power-domain"))
        .add_offer(clock_bus.serve(&mut service_fs, scope.to_handle(), "bus"))
        .add_offer(clock_regs.serve(&mut service_fs, scope.to_handle(), "registers"))
        .add_offer(reset.serve(&mut service_fs, scope.to_handle(), "reset"))
        .add_offer(gpio.serve(&mut service_fs, scope.to_handle(), "cs-0"))
        .set_driver_incoming(service_fs);

    let dispatcher = fdf_fidl::FidlExecutor::from(harness.dispatcher().clone());
    let started_driver = harness.start_driver().await.expect("Failed to start driver");

    let spi_service: fdf_component::ServiceInstance<fspiimpl::Service> =
        started_driver.driver_outgoing().service().connect_next().unwrap();

    let (client_end, server_end) = fdf_fidl::create_channel();
    spi_service.device(server_end).unwrap();
    let client = client_end.spawn_on(&dispatcher);

    // Create a VMO and register it.
    let vmo = Vmo::create(4096).unwrap();
    let dup_vmo = vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
    let range = fmem::natural::Range { vmo: vmo, offset: 0, size: 4096 };
    client
        .register_vmo(0, 1, range, SharedVmoRight::READ)
        .await
        .expect("FIDL call failed")
        .expect("Register VMO failed");

    // Registering the same VMO ID again should fail.
    let vmo = Vmo::create(4096).unwrap();
    let range = fmem::natural::Range { vmo: vmo, offset: 0, size: 4096 };
    let res =
        client.register_vmo(0, 1, range, SharedVmoRight::READ).await.expect("FIDL call failed");
    assert_eq!(res.unwrap_err(), Err(zx::Status::ALREADY_EXISTS));

    // Unregister the VMO and make sure it's the same one we registered.
    let unreg_vmo = client
        .unregister_vmo(0, 1)
        .await
        .expect("FIDL call failed")
        .expect("Unregister VMO failed")
        .vmo;
    assert_eq!(dup_vmo.koid().unwrap(), unreg_vmo.koid().unwrap());

    // Unregistering again should fail.
    let res = client.unregister_vmo(0, 1).await.expect("FIDL call failed");
    assert_eq!(res.unwrap_err(), Err(zx::Status::NOT_FOUND));

    let vmo = Vmo::create(4096).unwrap();
    let range = fmem::natural::Range { vmo: vmo, offset: 0, size: 4096 };
    client
        .register_vmo(0, 2, range, SharedVmoRight::READ)
        .await
        .expect("FIDL call failed")
        .expect("Register VMO failed");

    let vmo = Vmo::create(4096).unwrap();
    let range = fmem::natural::Range { vmo: vmo, offset: 0, size: 4096 };
    client
        .register_vmo(0, 3, range, SharedVmoRight::READ)
        .await
        .expect("FIDL call failed")
        .expect("Register VMO failed");

    // Release VMOs, and check that unregistering fails.
    client.release_registered_vmos(0).await.unwrap();

    let res = client.unregister_vmo(0, 2).await.expect("FIDL call failed");
    assert_eq!(res.unwrap_err(), Err(zx::Status::NOT_FOUND));

    let res = client.unregister_vmo(0, 3).await.expect("FIDL call failed");
    assert_eq!(res.unwrap_err(), Err(zx::Status::NOT_FOUND));

    started_driver.stop_driver().await;
}

#[fuchsia::test]
async fn test_vmo_registration_loopback() {
    let mut service_fs = ServiceFs::new();
    let scope = fasync::Scope::new_with_name("test");

    let vmo = Vmo::create(0x100).expect("Failed to create VMO");
    let mmio = fdevice::natural::Mmio { offset: Some(0), size: Some(0x100), vmo: Some(vmo) };

    let mut pdev_config: fake_pdev::Config = Default::default();
    pdev_config.mmios.insert(0, mmio);
    pdev_config.use_fake_irq = true;

    let pdev = FakePDev::new();
    pdev.set_config(pdev_config);

    let powerdomain = FakePowerDomain::new();
    let clock_bus = FakeClock::new();
    let clock_regs = FakeClock::new();
    let reset = FakeReset::new();
    let gpio = FakeGpio::default();

    let mut harness = TestHarness::<DwSpiDriver>::new()
        .add_offer(pdev.serve(&mut service_fs, scope.to_handle(), "pdev"))
        .add_offer(powerdomain.serve(&mut service_fs, scope.to_handle(), "power-domain"))
        .add_offer(clock_bus.serve(&mut service_fs, scope.to_handle(), "bus"))
        .add_offer(clock_regs.serve(&mut service_fs, scope.to_handle(), "registers"))
        .add_offer(reset.serve(&mut service_fs, scope.to_handle(), "reset"))
        .add_offer(gpio.serve(&mut service_fs, scope.to_handle(), "cs-0"))
        .set_driver_incoming(service_fs);

    let dispatcher = fdf_fidl::FidlExecutor::from(harness.dispatcher().clone());
    let started_driver = harness.start_driver().await.expect("Failed to start driver");

    let spi_service: fdf_component::ServiceInstance<fspiimpl::Service> =
        started_driver.driver_outgoing().service().connect_next().unwrap();

    let (client_end, server_end) = fdf_fidl::create_channel();
    spi_service.device(server_end).unwrap();
    let client = client_end.spawn_on(&dispatcher);

    // Create a VMO and register it on loopback.
    let vmo1 = Vmo::create(4096).unwrap();
    let dup_vmo1 = vmo1.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
    let range = fmem::natural::Range { vmo: vmo1, offset: 0, size: 4096 };
    client
        .register_vmo(fspiimpl::LOOPBACK_CHIP_SELECT, 1, range, SharedVmoRight::READ)
        .await
        .expect("FIDL call failed")
        .expect("Register VMO failed");

    // Registering the same VMO ID on normal CS should succeed (they are separate).
    let vmo2 = Vmo::create(4096).unwrap();
    let dup_vmo2 = vmo2.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
    let range = fmem::natural::Range { vmo: vmo2, offset: 0, size: 4096 };
    client
        .register_vmo(0, 1, range, SharedVmoRight::READ)
        .await
        .expect("FIDL call failed")
        .expect("Register VMO failed");

    // Unregister loopback VMO and verify it's vmo1.
    let unreg_vmo1 = client
        .unregister_vmo(fspiimpl::LOOPBACK_CHIP_SELECT, 1)
        .await
        .expect("FIDL call failed")
        .expect("Unregister VMO failed")
        .vmo;
    assert_eq!(dup_vmo1.koid().unwrap(), unreg_vmo1.koid().unwrap());

    // Unregister normal CS VMO and verify it's vmo2.
    let unreg_vmo2 = client
        .unregister_vmo(0, 1)
        .await
        .expect("FIDL call failed")
        .expect("Unregister VMO failed")
        .vmo;
    assert_eq!(dup_vmo2.koid().unwrap(), unreg_vmo2.koid().unwrap());

    started_driver.stop_driver().await;
}

#[fuchsia::test]
async fn test_transmit_vmo() {
    let mut service_fs = ServiceFs::new();
    let scope = fasync::Scope::new_with_name("test");

    let pdev = FakePDev::new();

    let vmo = Vmo::create(0x100).expect("Failed to create VMO");
    vmo.set_cache_policy(zx::CachePolicy::UnCachedDevice).expect("Failed to set cache policy");
    let mapping = mapped_vmo::Mapping::create_from_vmo(
        &vmo,
        0x100,
        zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE,
    )
    .expect("Failed to map VMO");

    let dup = vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("Failed to duplicate VMO");
    let irq = zx::VirtualInterrupt::create_virtual().expect("Failed to create virtual interrupt");
    let irq_dup =
        irq.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("Failed to duplicate interrupt");

    let mut pdev_config: fake_pdev::Config = Default::default();
    let mmio = fdevice::natural::Mmio { offset: Some(0), size: Some(0x100), vmo: Some(dup) };
    pdev_config.mmios.insert(0, mmio);
    pdev_config.irqs.insert(0, zx::Interrupt::from(irq.into_handle()));
    pdev.set_config(pdev_config);

    let powerdomain = FakePowerDomain::new();
    let clock_bus = FakeClock::new();
    let clock_regs = FakeClock::new();
    let reset = FakeReset::new();
    let gpio = FakeGpio::default();

    let mut harness = TestHarness::<DwSpiDriver>::new()
        .add_offer(pdev.serve(&mut service_fs, scope.to_handle(), "pdev"))
        .add_offer(powerdomain.serve(&mut service_fs, scope.to_handle(), "power-domain"))
        .add_offer(clock_bus.serve(&mut service_fs, scope.to_handle(), "bus"))
        .add_offer(clock_regs.serve(&mut service_fs, scope.to_handle(), "registers"))
        .add_offer(reset.serve(&mut service_fs, scope.to_handle(), "reset"))
        .add_offer(gpio.serve(&mut service_fs, scope.to_handle(), "cs-0"))
        .set_driver_incoming(service_fs);

    let dispatcher = fdf_fidl::FidlExecutor::from(harness.dispatcher().clone());
    let started_driver = harness.start_driver().await.expect("Failed to start driver");

    let spi_service: fdf_component::ServiceInstance<fspiimpl::Service> =
        started_driver.driver_outgoing().service().connect_next().unwrap();

    let (client_end, server_end) = fdf_fidl::create_channel();
    spi_service.device(server_end).unwrap();
    let client = client_end.spawn_on(&dispatcher);

    let read_u32 = |offset: usize| -> u32 {
        let mut bytes = [0u8; 4];
        mapping.read_at(offset, &mut bytes);
        u32::from_le_bytes(bytes)
    };

    let write_u32 = |offset: usize, value: u32| {
        mapping.write_at(offset, &value.to_le_bytes());
    };

    let tx_vmo = Vmo::create(4096).unwrap();
    let txdata = vec![1u8, 2, 3, 4, 5];
    tx_vmo.write(&txdata, 0).unwrap();
    let range = fmem::natural::Range { vmo: tx_vmo, offset: 0, size: 4096 };
    client
        .register_vmo(0, 1, range, SharedVmoRight::READ)
        .await
        .expect("FIDL call failed")
        .expect("Register VMO failed");

    // Set TXFLR and ISR.TXEIS to indicate TX FIFO empty, then trigger the interrupt.
    write_u32(0x20, 0x0);
    write_u32(0x30, 0x01);
    irq_dup.trigger(zx::BootInstant::ZERO).expect("Failed to trigger interrupt");

    let buffer = fsharedmemory::natural::SharedVmoBuffer { vmo_id: 1, offset: 0, size: 5 };
    let transmit_future = client.transmit_vmo(0, buffer);
    let transmit_future = std::pin::pin!(transmit_future);

    let untriggered_future =
        fasync::OnSignals::new(&irq_dup, zx::Signals::VIRTUAL_INTERRUPT_UNTRIGGERED);
    let untriggered_future = std::pin::pin!(untriggered_future);

    let transmit_future = match future::select(transmit_future, untriggered_future).await {
        Either::Left((res, _)) => {
            panic!("transmit_vmo completed prematurely: {:?}", res);
        }
        Either::Right((res, remaining_future)) => {
            res.expect("Failed to wait for untriggered signal");
            remaining_future
        }
    };

    // Set RXFLR and ISR.RXFIS to indicate that 5 bytes can be read from the RX FIFO, then trigger
    // the interrupt again.
    write_u32(0x24, 0x5);
    write_u32(0x30, 0x10);
    irq_dup.trigger(zx::BootInstant::ZERO).expect("Failed to trigger interrupt");

    let response = transmit_future.await;
    assert!(response.is_ok());
    assert!(response.unwrap().is_ok());

    // Verify what was written to the TX FIFO.
    assert_eq!(read_u32(0x60), 5);

    started_driver.stop_driver().await;
}

#[fuchsia::test]
async fn test_receive_vmo() {
    let mut service_fs = ServiceFs::new();
    let scope = fasync::Scope::new_with_name("test");

    let pdev = FakePDev::new();

    let vmo = Vmo::create(0x100).expect("Failed to create VMO");
    vmo.set_cache_policy(zx::CachePolicy::UnCachedDevice).expect("Failed to set cache policy");
    let mapping = mapped_vmo::Mapping::create_from_vmo(
        &vmo,
        0x100,
        zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE,
    )
    .expect("Failed to map VMO");

    let dup = vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("Failed to duplicate VMO");
    let irq = zx::VirtualInterrupt::create_virtual().expect("Failed to create virtual interrupt");
    let irq_dup =
        irq.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("Failed to duplicate interrupt");

    let mut pdev_config: fake_pdev::Config = Default::default();
    let mmio = fdevice::natural::Mmio { offset: Some(0), size: Some(0x100), vmo: Some(dup) };
    pdev_config.mmios.insert(0, mmio);
    pdev_config.irqs.insert(0, zx::Interrupt::from(irq.into_handle()));
    pdev.set_config(pdev_config);

    let powerdomain = FakePowerDomain::new();
    let clock_bus = FakeClock::new();
    let clock_regs = FakeClock::new();
    let reset = FakeReset::new();
    let gpio = FakeGpio::default();

    let mut harness = TestHarness::<DwSpiDriver>::new()
        .add_offer(pdev.serve(&mut service_fs, scope.to_handle(), "pdev"))
        .add_offer(powerdomain.serve(&mut service_fs, scope.to_handle(), "power-domain"))
        .add_offer(clock_bus.serve(&mut service_fs, scope.to_handle(), "bus"))
        .add_offer(clock_regs.serve(&mut service_fs, scope.to_handle(), "registers"))
        .add_offer(reset.serve(&mut service_fs, scope.to_handle(), "reset"))
        .add_offer(gpio.serve(&mut service_fs, scope.to_handle(), "cs-0"))
        .set_driver_incoming(service_fs);

    let dispatcher = fdf_fidl::FidlExecutor::from(harness.dispatcher().clone());
    let started_driver = harness.start_driver().await.expect("Failed to start driver");

    let spi_service: fdf_component::ServiceInstance<fspiimpl::Service> =
        started_driver.driver_outgoing().service().connect_next().unwrap();

    let (client_end, server_end) = fdf_fidl::create_channel();
    spi_service.device(server_end).unwrap();
    let client = client_end.spawn_on(&dispatcher);

    let write_u32 = |offset: usize, value: u32| {
        mapping.write_at(offset, &value.to_le_bytes());
    };

    let rx_vmo = Vmo::create(4096).unwrap();
    let dup_rx_vmo = rx_vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
    let range = fmem::natural::Range { vmo: rx_vmo, offset: 0, size: 4096 };
    client
        .register_vmo(0, 1, range, SharedVmoRight::WRITE)
        .await
        .expect("FIDL call failed")
        .expect("Register VMO failed");

    // Set TXFLR and ISR.TXEIS to indicate TX FIFO empty, then trigger the interrupt.
    write_u32(0x20, 0x0);
    write_u32(0x30, 0x01);
    irq_dup.trigger(zx::BootInstant::ZERO).expect("Failed to trigger interrupt");

    let buffer = fsharedmemory::natural::SharedVmoBuffer { vmo_id: 1, offset: 0, size: 5 };
    let receive_future = client.receive_vmo(0, buffer);
    let receive_future = std::pin::pin!(receive_future);

    let untriggered_future =
        fasync::OnSignals::new(&irq_dup, zx::Signals::VIRTUAL_INTERRUPT_UNTRIGGERED);
    let untriggered_future = std::pin::pin!(untriggered_future);

    let receive_future = match future::select(receive_future, untriggered_future).await {
        Either::Left((res, _)) => {
            panic!("receive_vmo completed prematurely: {:?}", res);
        }
        Either::Right((res, remaining_future)) => {
            res.expect("Failed to wait for untriggered signal");
            remaining_future
        }
    };

    // Set RXFLR and ISR.RXFIS to indicate that 5 bytes can be read from the RX FIFO, then trigger
    // the interrupt again.
    write_u32(0x24, 0x5);
    write_u32(0x30, 0x10);
    irq_dup.trigger(zx::BootInstant::ZERO).expect("Failed to trigger interrupt");

    let response = receive_future.await;
    assert!(response.is_ok());
    assert!(response.unwrap().is_ok());

    // Verify what was written to the VMO (0xFF because txdata was empty).
    let mut rx_data = vec![0u8; 5];
    dup_rx_vmo.read(&mut rx_data, 0).unwrap();
    assert_eq!(rx_data, vec![0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);

    started_driver.stop_driver().await;
}

#[fuchsia::test]
async fn test_exchange_vmo() {
    let mut service_fs = ServiceFs::new();
    let scope = fasync::Scope::new_with_name("test");

    let pdev = FakePDev::new();

    let vmo = Vmo::create(0x100).expect("Failed to create VMO");
    vmo.set_cache_policy(zx::CachePolicy::UnCachedDevice).expect("Failed to set cache policy");
    let mapping = mapped_vmo::Mapping::create_from_vmo(
        &vmo,
        0x100,
        zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE,
    )
    .expect("Failed to map VMO");

    let dup = vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("Failed to duplicate VMO");
    let irq = zx::VirtualInterrupt::create_virtual().expect("Failed to create virtual interrupt");
    let irq_dup =
        irq.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("Failed to duplicate interrupt");

    let mut pdev_config: fake_pdev::Config = Default::default();
    let mmio = fdevice::natural::Mmio { offset: Some(0), size: Some(0x100), vmo: Some(dup) };
    pdev_config.mmios.insert(0, mmio);
    pdev_config.irqs.insert(0, zx::Interrupt::from(irq.into_handle()));
    pdev.set_config(pdev_config);

    let powerdomain = FakePowerDomain::new();
    let clock_bus = FakeClock::new();
    let clock_regs = FakeClock::new();
    let reset = FakeReset::new();
    let gpio = FakeGpio::default();

    let mut harness = TestHarness::<DwSpiDriver>::new()
        .add_offer(pdev.serve(&mut service_fs, scope.to_handle(), "pdev"))
        .add_offer(powerdomain.serve(&mut service_fs, scope.to_handle(), "power-domain"))
        .add_offer(clock_bus.serve(&mut service_fs, scope.to_handle(), "bus"))
        .add_offer(clock_regs.serve(&mut service_fs, scope.to_handle(), "registers"))
        .add_offer(reset.serve(&mut service_fs, scope.to_handle(), "reset"))
        .add_offer(gpio.serve(&mut service_fs, scope.to_handle(), "cs-0"))
        .set_driver_incoming(service_fs);

    let dispatcher = fdf_fidl::FidlExecutor::from(harness.dispatcher().clone());
    let started_driver = harness.start_driver().await.expect("Failed to start driver");

    let spi_service: fdf_component::ServiceInstance<fspiimpl::Service> =
        started_driver.driver_outgoing().service().connect_next().unwrap();

    let (client_end, server_end) = fdf_fidl::create_channel();
    spi_service.device(server_end).unwrap();
    let client = client_end.spawn_on(&dispatcher);

    let write_u32 = |offset: usize, value: u32| {
        mapping.write_at(offset, &value.to_le_bytes());
    };

    // Create TX VMO and write [1, 2, 3, 4, 5].
    let tx_vmo = Vmo::create(4096).unwrap();
    tx_vmo.write(&[1u8, 2, 3, 4, 5], 0).unwrap();
    let tx_range = fmem::natural::Range { vmo: tx_vmo, offset: 0, size: 4096 };
    client.register_vmo(0, 1, tx_range, SharedVmoRight::READ).await.unwrap().unwrap();

    // Create RX VMO.
    let rx_vmo = Vmo::create(4096).unwrap();
    let dup_rx_vmo = rx_vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
    let rx_range = fmem::natural::Range { vmo: rx_vmo, offset: 0, size: 4096 };
    client.register_vmo(0, 2, rx_range, SharedVmoRight::WRITE).await.unwrap().unwrap();

    // Set TXFLR and ISR.TXEIS to indicate TX FIFO empty, then trigger the interrupt.
    write_u32(0x20, 0x0);
    write_u32(0x30, 0x01);
    irq_dup.trigger(zx::BootInstant::ZERO).expect("Failed to trigger interrupt");

    let tx_buffer = fsharedmemory::natural::SharedVmoBuffer { vmo_id: 1, offset: 0, size: 5 };
    let rx_buffer = fsharedmemory::natural::SharedVmoBuffer { vmo_id: 2, offset: 0, size: 5 };
    let exchange_future = client.exchange_vmo(0, tx_buffer, rx_buffer);
    let exchange_future = std::pin::pin!(exchange_future);

    let untriggered_future =
        fasync::OnSignals::new(&irq_dup, zx::Signals::VIRTUAL_INTERRUPT_UNTRIGGERED);
    let untriggered_future = std::pin::pin!(untriggered_future);

    let exchange_future = match future::select(exchange_future, untriggered_future).await {
        Either::Left((res, _)) => {
            panic!("exchange_vmo completed prematurely: {:?}", res);
        }
        Either::Right((res, remaining_future)) => {
            res.expect("Failed to wait for untriggered signal");
            remaining_future
        }
    };

    // Set RXFLR and ISR.RXFIS to indicate that 5 bytes can be read from the RX FIFO, then trigger
    // the interrupt again.
    write_u32(0x24, 0x5);
    write_u32(0x30, 0x10);
    irq_dup.trigger(zx::BootInstant::ZERO).expect("Failed to trigger interrupt");

    let response = exchange_future.await;
    assert!(response.is_ok());
    assert!(response.unwrap().is_ok());

    // Check RX VMO content. Last written byte was 5.
    let mut rx_data = vec![0u8; 5];
    dup_rx_vmo.read(&mut rx_data, 0).unwrap();
    assert_eq!(rx_data, vec![5, 5, 5, 5, 5]);

    started_driver.stop_driver().await;
}

#[fuchsia::test]
async fn test_vmo_validation() {
    let mut service_fs = ServiceFs::new();
    let scope = fasync::Scope::new_with_name("test");

    let pdev = FakePDev::new();

    let vmo = Vmo::create(0x100).expect("Failed to create VMO");
    let dup = vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("Failed to duplicate VMO");
    let irq = zx::VirtualInterrupt::create_virtual().expect("Failed to create virtual interrupt");

    let mut pdev_config: fake_pdev::Config = Default::default();
    let mmio = fdevice::natural::Mmio { offset: Some(0), size: Some(0x100), vmo: Some(dup) };
    pdev_config.mmios.insert(0, mmio);
    pdev_config.irqs.insert(0, zx::Interrupt::from(irq.into_handle()));
    pdev.set_config(pdev_config);

    let powerdomain = FakePowerDomain::new();
    let clock_bus = FakeClock::new();
    let clock_regs = FakeClock::new();
    let reset = FakeReset::new();
    let gpio = FakeGpio::default();

    let mut harness = TestHarness::<DwSpiDriver>::new()
        .add_offer(pdev.serve(&mut service_fs, scope.to_handle(), "pdev"))
        .add_offer(powerdomain.serve(&mut service_fs, scope.to_handle(), "power-domain"))
        .add_offer(clock_bus.serve(&mut service_fs, scope.to_handle(), "bus"))
        .add_offer(clock_regs.serve(&mut service_fs, scope.to_handle(), "registers"))
        .add_offer(reset.serve(&mut service_fs, scope.to_handle(), "reset"))
        .add_offer(gpio.serve(&mut service_fs, scope.to_handle(), "cs-0"))
        .set_driver_incoming(service_fs);

    let dispatcher = fdf_fidl::FidlExecutor::from(harness.dispatcher().clone());
    let started_driver = harness.start_driver().await.expect("Failed to start driver");

    let spi_service: fdf_component::ServiceInstance<fspiimpl::Service> =
        started_driver.driver_outgoing().service().connect_next().unwrap();

    let (client_end, server_end) = fdf_fidl::create_channel();
    spi_service.device(server_end).unwrap();
    let client = client_end.spawn_on(&dispatcher);

    let vmo = Vmo::create(4096).unwrap();
    let range = fmem::natural::Range { vmo: vmo, offset: 0, size: 1024 }; // only register 1024 bytes

    client.register_vmo(0, 1, range, SharedVmoRight::READ).await.unwrap().unwrap();

    // Invalid VMO ID
    let buffer = fsharedmemory::natural::SharedVmoBuffer { vmo_id: 99, offset: 0, size: 5 };
    let res = client.transmit_vmo(0, buffer).await.unwrap();
    assert_eq!(res.unwrap_err(), Err(zx::Status::NOT_FOUND));

    // Out of range (offset + size > registered size)
    let buffer = fsharedmemory::natural::SharedVmoBuffer { vmo_id: 1, offset: 1020, size: 10 };
    let res = client.transmit_vmo(0, buffer).await.unwrap();
    assert_eq!(res.unwrap_err(), Err(zx::Status::OUT_OF_RANGE));

    // Out of range (overflow)
    let buffer =
        fsharedmemory::natural::SharedVmoBuffer { vmo_id: 1, offset: u64::MAX - 2, size: 10 };
    let res = client.transmit_vmo(0, buffer).await.unwrap();
    assert_eq!(res.unwrap_err(), Err(zx::Status::OUT_OF_RANGE));

    started_driver.stop_driver().await;
}

#[fuchsia::test]
async fn test_loopback_mode() {
    let mut service_fs = ServiceFs::new();
    let scope = fasync::Scope::new_with_name("test");

    let pdev = FakePDev::new();

    let vmo = Vmo::create(0x100).expect("Failed to create VMO");
    vmo.set_cache_policy(zx::CachePolicy::UnCachedDevice).expect("Failed to set cache policy");
    let mapping = mapped_vmo::Mapping::create_from_vmo(
        &vmo,
        0x100,
        zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE,
    )
    .expect("Failed to map VMO");

    let dup = vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("Failed to duplicate VMO");
    let irq = zx::VirtualInterrupt::create_virtual().expect("Failed to create virtual interrupt");
    let irq_dup =
        irq.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("Failed to duplicate interrupt");

    let mut pdev_config: fake_pdev::Config = Default::default();
    let mmio = fdevice::natural::Mmio { offset: Some(0), size: Some(0x100), vmo: Some(dup) };
    pdev_config.mmios.insert(0, mmio);
    pdev_config.irqs.insert(0, zx::Interrupt::from(irq.into_handle()));
    pdev.set_config(pdev_config);

    let powerdomain = FakePowerDomain::new();
    let clock_bus = FakeClock::new();
    let clock_regs = FakeClock::new();
    let reset = FakeReset::new();

    let gpio = FakeGpio::default();

    let mut harness = TestHarness::<DwSpiDriver>::new()
        .add_offer(pdev.serve(&mut service_fs, scope.to_handle(), "pdev"))
        .add_offer(powerdomain.serve(&mut service_fs, scope.to_handle(), "power-domain"))
        .add_offer(clock_bus.serve(&mut service_fs, scope.to_handle(), "bus"))
        .add_offer(clock_regs.serve(&mut service_fs, scope.to_handle(), "registers"))
        .add_offer(reset.serve(&mut service_fs, scope.to_handle(), "reset"))
        .add_offer(gpio.serve(&mut service_fs, scope.to_handle(), "cs-0"))
        .set_driver_incoming(service_fs);

    let dispatcher = fdf_fidl::FidlExecutor::from(harness.dispatcher().clone());
    let started_driver = harness.start_driver().await.expect("Failed to start driver");

    let spi_service: fdf_component::ServiceInstance<fspiimpl::Service> =
        started_driver.driver_outgoing().service().connect_next().unwrap();

    let (client_end, server_end) = fdf_fidl::create_channel();
    spi_service.device(server_end).unwrap();
    let client = client_end.spawn_on(&dispatcher);

    // 1. Normal transfer (CS 0)
    perform_transfer(&client, &irq_dup, &gpio, &mapping, 0).await;

    // 2. Loopback transfer (CS LOOPBACK_CHIP_SELECT)
    perform_transfer(&client, &irq_dup, &gpio, &mapping, fspiimpl::LOOPBACK_CHIP_SELECT).await;

    // 3. Normal transfer (CS 0)
    perform_transfer(&client, &irq_dup, &gpio, &mapping, 0).await;

    started_driver.stop_driver().await;
}

async fn perform_transfer(
    client: &fidl_next::Client<fspiimpl::SpiImpl, fdf_fidl::DriverChannel>,
    irq_dup: &zx::VirtualInterrupt,
    gpio: &FakeGpio,
    mapping: &mapped_vmo::Mapping,
    cs: u32,
) {
    let write_u32 = |offset: usize, value: u32| {
        mapping.write_at(offset, &value.to_le_bytes());
    };
    let read_u32 = |offset: usize| -> u32 {
        let mut bytes = [0u8; 4];
        mapping.read_at(offset, &mut bytes);
        u32::from_le_bytes(bytes)
    };

    write_u32(0x24, 0x0);
    write_u32(0x20, 0x0);
    write_u32(0x30, 0x01);
    irq_dup.trigger(zx::BootInstant::from_nanos(1)).expect("Failed to trigger interrupt");

    let exchange_future = client.exchange_vector(cs, vec![1u8, 2, 3, 4, 5]);
    let exchange_future = std::pin::pin!(exchange_future);

    let untriggered_future =
        fasync::OnSignals::new(irq_dup, zx::Signals::VIRTUAL_INTERRUPT_UNTRIGGERED);
    let untriggered_future = std::pin::pin!(untriggered_future);

    let exchange_future = match future::select(exchange_future, untriggered_future).await {
        Either::Left((res, _)) => {
            panic!("exchange_vector completed prematurely: {:?}", res);
        }
        Either::Right((res, remaining_exchange_future)) => {
            res.expect("Failed to wait for untriggered signal");
            remaining_exchange_future
        }
    };

    // During transfer:
    // - SRL should be 1 if loopback, 0 otherwise
    // - CS should be High (deasserted) if loopback, Low (asserted) otherwise
    let ctrlr0 = read_u32(0x0);
    let expected_srl = cs == fspiimpl::LOOPBACK_CHIP_SELECT;
    assert_eq!(registers::CtrlR0::from_raw(ctrlr0).srl(), expected_srl);

    let expected_buffer_mode = if cs == fspiimpl::LOOPBACK_CHIP_SELECT {
        fgpio::BufferMode::OutputHigh
    } else {
        fgpio::BufferMode::OutputLow
    };
    assert_eq!(gpio.buffer_mode(), expected_buffer_mode);

    write_u32(0x24, 0x5);
    write_u32(0x30, 0x10);
    irq_dup.trigger(zx::BootInstant::from_nanos(1)).expect("Failed to trigger interrupt");

    let _ = exchange_future.await.expect("exchange_vector failed");

    // After transfer:
    // - SRL should be the same as before
    // - CS should be High (deasserted)
    let ctrlr0 = read_u32(0x0);
    assert_eq!(registers::CtrlR0::from_raw(ctrlr0).srl(), expected_srl);
    assert_eq!(gpio.buffer_mode(), fgpio::BufferMode::OutputHigh);
}
