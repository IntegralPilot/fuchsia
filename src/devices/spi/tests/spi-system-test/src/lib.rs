// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use fidl::endpoints::ServiceMarker;
use fidl_fuchsia_driver_framework as fdf;
use fidl_fuchsia_driver_token as ftoken;
use fidl_fuchsia_hardware_sharedmemory as fsharedmemory;
use fidl_fuchsia_hardware_spi as fspi;
use fidl_fuchsia_mem as fmem;
use fuchsia_component::client::{connect_to_service_instance, open_service_at};
use fuchsia_fs::directory::{WatchEvent, Watcher};
use futures::StreamExt;
use rand::RngExt as _;
use spi_system_test_config::Config;
use std::collections::{HashMap, HashSet};
use zx::Status;

#[derive(Clone, Debug)]
struct SpiBus {
    min_expected_throughput: u64,
    max_expected_latency: u64,
}

/// Returns the most specific address that is stable, or `None` if no such address exists. Only
/// string addresses are supported for now.
fn get_bus_address(path: Vec<fdf::BusInfo>) -> Option<String> {
    for info in path.into_iter().rev() {
        if info.address_stability != Some(fdf::DeviceAddressStability::Stable) {
            continue;
        }

        if let Some(address) = info.address {
            return match address {
                fdf::DeviceAddress::StringValue(val) => Some(val),
                _ => None,
            };
        }
    }
    None
}

/// Connects to a SPI bus, gets its address, and establishes a loopback connection. `None` is
/// returned if the bus does not have a stable address.
async fn try_connect_to_bus(
    instance_name: &str,
    topology_proxy: &ftoken::NodeBusTopologyProxy,
) -> Result<Option<(String, fspi::DeviceProxy)>> {
    let service_proxy = connect_to_service_instance::<fspi::TestServiceMarker>(instance_name)
        .context("Failed to connect to TestService instance")?;
    let test_proxy =
        service_proxy.connect_to_test().context("Failed to connect to test protocol")?;

    let token = test_proxy
        .get()
        .await
        .context("Get FIDL call failed")?
        .map_err(|status| anyhow::anyhow!("Get failed: {:?}", Status::err_from_raw(status)))?;

    let path =
        topology_proxy.get(token).await.context("NodeBusTopology.Get FIDL call failed")?.map_err(
            |status| {
                anyhow::anyhow!("NodeBusTopology.Get failed: {:?}", Status::err_from_raw(status))
            },
        )?;

    let Some(bus_address) = get_bus_address(path) else {
        return Ok(None);
    };

    let (device_client, device_server) = fidl::endpoints::create_proxy::<fspi::DeviceMarker>();
    test_proxy
        .connect_spi_loopback(device_server)
        .await
        .context("ConnectSpiLoopback FIDL call failed")?
        .map_err(|status| anyhow::anyhow!("ConnectSpiLoopback failed: {:?}", status))?;
    return Ok(Some((bus_address, device_client)));
}

/// Discovers SPI devices matching the specified bus addresses, waiting for all of them to appear.
async fn discover_devices(bus_addresses: &[String]) -> Result<HashMap<String, fspi::DeviceProxy>> {
    if bus_addresses.is_empty() {
        return Ok(HashMap::new());
    }

    let topology_proxy =
        fuchsia_component::client::connect_to_protocol::<ftoken::NodeBusTopologyMarker>()
            .context("Failed to connect to NodeBusTopology")?;

    let mut remaining_addresses: HashSet<String> = bus_addresses.iter().cloned().collect();

    let service_directory = open_service_at(fspi::TestServiceMarker::SERVICE_NAME)
        .context("Failed to open service directory")?;
    let mut watcher = Watcher::new(&service_directory).await.context("Failed to create watcher")?;
    let mut devices = HashMap::new();

    while let Some(message) = watcher.next().await {
        let message = message.context("Watcher error")?;
        match message.event {
            WatchEvent::EXISTING | WatchEvent::ADD_FILE => {
                let filename = message.filename.to_str().ok_or_else(|| {
                    anyhow::anyhow!("Invalid UTF-8 in filename: {:?}", message.filename)
                })?;
                if filename == "." || filename == ".." {
                    continue;
                }

                match try_connect_to_bus(filename, &topology_proxy).await {
                    Ok(Some((bus_address, device_proxy))) => {
                        // Ignore buses that are not in the list of expected addresses.
                        if remaining_addresses.remove(&bus_address) {
                            devices.insert(bus_address, device_proxy);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }

        if remaining_addresses.is_empty() {
            return Ok(devices);
        }
    }

    anyhow::bail!(
        "Watcher stream ended before finding all expected SPI buses. Found {} of {}",
        devices.len(),
        bus_addresses.len(),
    );
}

async fn run_with_devices<F, Fut>(test_func: F) -> Result<()>
where
    F: Fn(fspi::DeviceProxy, SpiBus) -> Fut + Sync + Send,
    Fut: futures::Future<Output = Result<()>> + Send,
{
    static CONFIG: std::sync::OnceLock<Config> = std::sync::OnceLock::new();
    let config = CONFIG.get_or_init(|| Config::take_from_startup_handle());

    if config.bus_addresses.len() != config.min_expected_throughput_bytes_per_second.len() {
        anyhow::bail!(
            "bus_addresses (len {}) and min_expected_throughput_bytes_per_second (len {}) must have the same length",
            config.bus_addresses.len(),
            config.min_expected_throughput_bytes_per_second.len()
        );
    }

    if config.bus_addresses.len() != config.max_expected_latency_nanoseconds.len() {
        anyhow::bail!(
            "bus_addresses (len {}) and max_expected_latency_nanoseconds (len {}) must have the same length",
            config.bus_addresses.len(),
            config.max_expected_latency_nanoseconds.len()
        );
    }

    let devices_map = discover_devices(&config.bus_addresses).await?;

    let bus_configs: HashMap<String, SpiBus> = config
        .bus_addresses
        .iter()
        .zip(&config.min_expected_throughput_bytes_per_second)
        .zip(&config.max_expected_latency_nanoseconds)
        .map(|((name, &min_expected_throughput), &max_expected_latency)| {
            (name.clone(), SpiBus { min_expected_throughput, max_expected_latency })
        })
        .collect();

    let test_func = &test_func;
    let futures = devices_map.into_iter().map(|(bus_address, device)| {
        let bus_config = bus_configs.get(&bus_address).cloned().expect("Could not find bus");
        async move {
            test_func(device, bus_config)
                .await
                .with_context(|| format!("Test failed for bus {}", bus_address))
        }
    });

    let results = futures::future::join_all(futures).await;
    let errors: Vec<_> = results.into_iter().filter_map(|r| r.err()).collect();
    assert!(errors.is_empty(), "Some tests failed: {:?}", errors);

    Ok(())
}

// Helper macro to define a test case that runs for every available SPI bus.
macro_rules! spi_test {
    ($name:ident, $device:ident, $body:block) => {
        #[fuchsia::test]
        async fn $name() -> Result<()> {
            run_with_devices(|$device, _bus| async move { $body }).await
        }
    };
    ($name:ident, $device:ident, $bus:ident, $body:block) => {
        #[fuchsia::test]
        async fn $name() -> Result<()> {
            run_with_devices(|$device, $bus| async move { $body }).await
        }
    };
}

spi_test!(test_transmit_vector, device, {
    const BUFFER_SIZE: usize = 512;

    let mut txdata = vec![0u8; BUFFER_SIZE];
    rand::rng().fill(&mut txdata[..]);

    let status =
        device.transmit_vector(&txdata).await.context("TransmitVector FIDL call failed")?;

    assert_eq!(Status::ok(status), Ok(()));
    Ok(())
});

spi_test!(test_receive_vector, device, {
    const BUFFER_SIZE: usize = 512;

    let (status, rxdata) = device
        .receive_vector(BUFFER_SIZE as u32)
        .await
        .context("ReceiveVector FIDL call failed")?;

    assert_eq!(Status::ok(status), Ok(()));
    assert_eq!(rxdata.len(), BUFFER_SIZE);
    Ok(())
});

spi_test!(test_exchange_vector, device, {
    const BUFFER_SIZE: usize = 512;

    let mut txdata = vec![0u8; BUFFER_SIZE];
    rand::rng().fill(&mut txdata[..]);

    let (status, rxdata) =
        device.exchange_vector(&txdata).await.context("ExchangeVector FIDL call failed")?;

    assert_eq!(Status::ok(status), Ok(()));
    assert_eq!(txdata, rxdata);
    Ok(())
});

spi_test!(test_exchange_vector_multiple, device, {
    const BUFFER_SIZE: usize = 512;
    const CONCURRENT_REQUESTS: usize = 10;

    let mut futures = Vec::new();
    for _ in 0..CONCURRENT_REQUESTS {
        let mut txdata = vec![0u8; BUFFER_SIZE];
        rand::rng().fill(&mut txdata[..]);
        let device_clone = device.clone();

        futures.push(async move {
            let (status, rxdata) = device_clone
                .exchange_vector(&txdata)
                .await
                .context("ExchangeVector FIDL call failed")?;
            Ok::<_, anyhow::Error>((txdata, rxdata, status))
        });
    }

    let results = futures::future::try_join_all(futures).await?;
    for (txdata, rxdata, status) in results {
        assert_eq!(Status::ok(status), Ok(()));
        assert_eq!(txdata, rxdata);
    }
    Ok(())
});

spi_test!(test_transmit_vmo, device, {
    const BUFFER_SIZE: usize = 512;
    const VMO_ID: u32 = 1;

    let mut txdata = vec![0u8; BUFFER_SIZE];
    rand::rng().fill(&mut txdata[..]);

    let vmo = zx::Vmo::create(BUFFER_SIZE as u64).context("Failed to create VMO")?;
    vmo.write(&txdata, 0).context("Failed to write to VMO")?;

    let vmo_dup =
        vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).context("Failed to duplicate VMO")?;

    device
        .register_vmo(
            VMO_ID,
            fmem::Range { vmo: vmo_dup, offset: 0, size: BUFFER_SIZE as u64 },
            fsharedmemory::SharedVmoRight::READ,
        )
        .await
        .context("RegisterVmo FIDL call failed")?
        .map_err(|status| {
            anyhow::anyhow!("RegisterVmo failed: {:?}", Status::err_from_raw(status))
        })?;

    device
        .transmit(&fsharedmemory::SharedVmoBuffer {
            vmo_id: VMO_ID,
            offset: 0,
            size: BUFFER_SIZE as u64,
        })
        .await
        .context("Transmit FIDL call failed")?
        .map_err(|status| anyhow::anyhow!("Transmit failed: {:?}", Status::err_from_raw(status)))?;

    let _unregistered_vmo =
        device.unregister_vmo(VMO_ID).await.context("UnregisterVmo FIDL call failed")?.map_err(
            |status| anyhow::anyhow!("UnregisterVmo failed: {:?}", Status::err_from_raw(status)),
        )?;

    Ok(())
});

spi_test!(test_receive_vmo, device, {
    const BUFFER_SIZE: usize = 512;
    const VMO_ID: u32 = 1;

    let vmo = zx::Vmo::create(BUFFER_SIZE as u64).context("Failed to create VMO")?;
    let vmo_dup =
        vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).context("Failed to duplicate VMO")?;

    device
        .register_vmo(
            VMO_ID,
            fmem::Range { vmo: vmo_dup, offset: 0, size: BUFFER_SIZE as u64 },
            fsharedmemory::SharedVmoRight::WRITE,
        )
        .await
        .context("RegisterVmo FIDL call failed")?
        .map_err(|status| {
            anyhow::anyhow!("RegisterVmo failed: {:?}", Status::err_from_raw(status))
        })?;

    device
        .receive(&fsharedmemory::SharedVmoBuffer {
            vmo_id: VMO_ID,
            offset: 0,
            size: BUFFER_SIZE as u64,
        })
        .await
        .context("Receive FIDL call failed")?
        .map_err(|status| anyhow::anyhow!("Receive failed: {:?}", Status::err_from_raw(status)))?;

    let _unregistered_vmo =
        device.unregister_vmo(VMO_ID).await.context("UnregisterVmo FIDL call failed")?.map_err(
            |status| anyhow::anyhow!("UnregisterVmo failed: {:?}", Status::err_from_raw(status)),
        )?;

    Ok(())
});

spi_test!(test_exchange_vmo, device, {
    const BUFFER_SIZE: usize = 512;
    const TX_VMO_ID: u32 = 1;
    const RX_VMO_ID: u32 = 2;

    let mut txdata = vec![0u8; BUFFER_SIZE];
    rand::rng().fill(&mut txdata[..]);

    let tx_vmo = zx::Vmo::create(BUFFER_SIZE as u64).context("Failed to create TX VMO")?;
    tx_vmo.write(&txdata, 0).context("Failed to write to TX VMO")?;
    let tx_vmo_dup =
        tx_vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).context("Failed to duplicate TX VMO")?;

    let rx_vmo = zx::Vmo::create(BUFFER_SIZE as u64).context("Failed to create RX VMO")?;
    let rx_vmo_dup =
        rx_vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).context("Failed to duplicate RX VMO")?;

    device
        .register_vmo(
            TX_VMO_ID,
            fmem::Range { vmo: tx_vmo_dup, offset: 0, size: BUFFER_SIZE as u64 },
            fsharedmemory::SharedVmoRight::READ,
        )
        .await
        .context("RegisterVmo TX FIDL call failed")?
        .map_err(|status| {
            anyhow::anyhow!("RegisterVmo TX failed: {:?}", Status::err_from_raw(status))
        })?;

    device
        .register_vmo(
            RX_VMO_ID,
            fmem::Range { vmo: rx_vmo_dup, offset: 0, size: BUFFER_SIZE as u64 },
            fsharedmemory::SharedVmoRight::WRITE,
        )
        .await
        .context("RegisterVmo RX FIDL call failed")?
        .map_err(|status| {
            anyhow::anyhow!("RegisterVmo RX failed: {:?}", Status::err_from_raw(status))
        })?;

    device
        .exchange(
            &fsharedmemory::SharedVmoBuffer {
                vmo_id: TX_VMO_ID,
                offset: 0,
                size: BUFFER_SIZE as u64,
            },
            &fsharedmemory::SharedVmoBuffer {
                vmo_id: RX_VMO_ID,
                offset: 0,
                size: BUFFER_SIZE as u64,
            },
        )
        .await
        .context("Exchange FIDL call failed")?
        .map_err(|status| anyhow::anyhow!("Exchange failed: {:?}", Status::err_from_raw(status)))?;

    let mut rxdata = vec![0u8; BUFFER_SIZE];
    rx_vmo.read(&mut rxdata, 0).context("Failed to read from RX VMO")?;
    assert_eq!(txdata, rxdata);

    let _unregistered_tx_vmo = device
        .unregister_vmo(TX_VMO_ID)
        .await
        .context("UnregisterVmo TX FIDL call failed")?
        .map_err(|status| {
            anyhow::anyhow!("UnregisterVmo TX failed: {:?}", Status::err_from_raw(status))
        })?;

    let _unregistered_rx_vmo = device
        .unregister_vmo(RX_VMO_ID)
        .await
        .context("UnregisterVmo RX FIDL call failed")?
        .map_err(|status| {
            anyhow::anyhow!("UnregisterVmo RX failed: {:?}", Status::err_from_raw(status))
        })?;

    Ok(())
});

spi_test!(test_exchange_vmo_multiple, device, {
    const BUFFER_SIZE: usize = 512;
    const CONCURRENT_REQUESTS: usize = 10;
    const TX_VMO_ID: u32 = 1;
    const RX_VMO_ID: u32 = 2;
    const TOTAL_SIZE: usize = BUFFER_SIZE * CONCURRENT_REQUESTS;

    let tx_vmo = zx::Vmo::create(TOTAL_SIZE as u64).context("Failed to create TX VMO")?;
    let rx_vmo = zx::Vmo::create(TOTAL_SIZE as u64).context("Failed to create RX VMO")?;

    let mut txdata_all = vec![0u8; TOTAL_SIZE];
    rand::rng().fill(&mut txdata_all[..]);
    tx_vmo.write(&txdata_all, 0).context("Failed to write to TX VMO")?;

    let tx_vmo_dup =
        tx_vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).context("Failed to duplicate TX VMO")?;
    let rx_vmo_dup =
        rx_vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).context("Failed to duplicate RX VMO")?;

    device
        .register_vmo(
            TX_VMO_ID,
            fmem::Range { vmo: tx_vmo_dup, offset: 0, size: TOTAL_SIZE as u64 },
            fsharedmemory::SharedVmoRight::READ,
        )
        .await
        .context("RegisterVmo TX FIDL call failed")?
        .map_err(|status| {
            anyhow::anyhow!("RegisterVmo TX failed: {:?}", Status::err_from_raw(status))
        })?;

    device
        .register_vmo(
            RX_VMO_ID,
            fmem::Range { vmo: rx_vmo_dup, offset: 0, size: TOTAL_SIZE as u64 },
            fsharedmemory::SharedVmoRight::WRITE,
        )
        .await
        .context("RegisterVmo RX FIDL call failed")?
        .map_err(|status| {
            anyhow::anyhow!("RegisterVmo RX failed: {:?}", Status::err_from_raw(status))
        })?;

    let mut futures = Vec::new();
    for i in 0..CONCURRENT_REQUESTS {
        let offset = (i * BUFFER_SIZE) as u64;
        let device_clone = device.clone();

        futures.push(async move {
            device_clone
                .exchange(
                    &fsharedmemory::SharedVmoBuffer {
                        vmo_id: TX_VMO_ID,
                        offset,
                        size: BUFFER_SIZE as u64,
                    },
                    &fsharedmemory::SharedVmoBuffer {
                        vmo_id: RX_VMO_ID,
                        offset,
                        size: BUFFER_SIZE as u64,
                    },
                )
                .await
                .context("Exchange FIDL call failed")?
                .map_err(|status| {
                    anyhow::anyhow!("Exchange failed: {:?}", Status::err_from_raw(status))
                })?;
            Ok::<_, anyhow::Error>(())
        });
    }

    futures::future::try_join_all(futures).await?;

    let mut rxdata_all = vec![0u8; TOTAL_SIZE];
    rx_vmo.read(&mut rxdata_all, 0).context("Failed to read from RX VMO")?;
    assert_eq!(txdata_all, rxdata_all);

    device.unregister_vmo(TX_VMO_ID).await.context("UnregisterVmo TX FIDL call failed")?.map_err(
        |status| anyhow::anyhow!("UnregisterVmo TX failed: {:?}", Status::err_from_raw(status)),
    )?;

    device.unregister_vmo(RX_VMO_ID).await.context("UnregisterVmo RX FIDL call failed")?.map_err(
        |status| anyhow::anyhow!("UnregisterVmo RX failed: {:?}", Status::err_from_raw(status)),
    )?;

    Ok(())
});

// This test is intended to stress the SPI controller to find erroneous conditions such as data
// corruption, unexpected interrupts, or timeouts. It makes continuous calls to Exchange() for ten
// seconds, with as many as ten requests pending at once. The test fails if the SPI controller
// driver returns an error or the received data does not match what was sent.
spi_test!(test_stress, device, {
    // The first 10 VMOs are for TX, the rest are for RX. VMO IDs start at zero.
    const MAX_CONCURRENT_CALLS: usize = 10;
    const VMO_COUNT: usize = MAX_CONCURRENT_CALLS * 2;
    const VMO_SIZE: usize = 4096;
    const TEST_DURATION: std::time::Duration = std::time::Duration::from_secs(10);

    let mut vmos = Vec::with_capacity(VMO_COUNT);
    for i in 0..VMO_COUNT {
        let vmo_id = i as u32;
        let vmo = zx::Vmo::create(VMO_SIZE as u64).context("Failed to create VMO")?;
        let vmo_dup =
            vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).context("Failed to duplicate VMO")?;

        let rights = if i < MAX_CONCURRENT_CALLS {
            fsharedmemory::SharedVmoRight::READ
        } else {
            fsharedmemory::SharedVmoRight::WRITE
        };

        device
            .register_vmo(
                vmo_id,
                fmem::Range { vmo: vmo_dup, offset: 0, size: VMO_SIZE as u64 },
                rights,
            )
            .await
            .context("RegisterVmo FIDL call failed")?
            .map_err(|status| {
                anyhow::anyhow!("RegisterVmo failed: {:?}", Status::err_from_raw(status))
            })?;

        vmos.push(vmo);
    }

    let make_request = |slot: usize| {
        let tx_vmo_id = slot as u32;
        let rx_vmo_id = tx_vmo_id + MAX_CONCURRENT_CALLS as u32;
        let tx_vmo = &vmos[tx_vmo_id as usize];
        let rx_vmo = &vmos[rx_vmo_id as usize];
        let device_clone = device.clone();

        async move {
            let mut txdata = vec![0u8; VMO_SIZE];
            rand::rng().fill(&mut txdata[..]);
            tx_vmo.write(&txdata, 0).context("Failed to write to TX VMO")?;

            device_clone
                .exchange(
                    &fsharedmemory::SharedVmoBuffer {
                        vmo_id: tx_vmo_id,
                        offset: 0,
                        size: VMO_SIZE as u64,
                    },
                    &fsharedmemory::SharedVmoBuffer {
                        vmo_id: rx_vmo_id,
                        offset: 0,
                        size: VMO_SIZE as u64,
                    },
                )
                .await
                .context("Exchange FIDL call failed")?
                .map_err(|status| {
                    anyhow::anyhow!("Exchange failed: {:?}", Status::err_from_raw(status))
                })?;

            let mut rxdata = vec![0u8; VMO_SIZE];
            rx_vmo.read(&mut rxdata, 0).context("Failed to read from RX VMO")?;
            assert_eq!(txdata, rxdata);

            Ok::<_, anyhow::Error>(slot)
        }
    };

    let start_time = std::time::Instant::now();
    let mut in_flight = futures::stream::FuturesUnordered::new();
    for slot in 0..MAX_CONCURRENT_CALLS {
        in_flight.push(make_request(slot));
    }

    while let Some(result) = in_flight.next().await {
        let slot = result?;
        if start_time.elapsed() < TEST_DURATION {
            in_flight.push(make_request(slot));
        }
    }

    for i in 0..VMO_COUNT {
        let vmo_id = i as u32;
        let _unregistered_vmo = device
            .unregister_vmo(vmo_id)
            .await
            .context("UnregisterVmo FIDL call failed")?
            .map_err(|status| {
                anyhow::anyhow!("UnregisterVmo failed: {:?}", Status::err_from_raw(status))
            })?;
    }

    Ok(())
});

// This test measures the throughput of a large call to Exchange() in bytes per second, which is
// then compared to a minimum expected value for this device. The test passes if the actual
// throughput is greater than or equal to the expected throughput. The expected throughput may be a
// hard requirement for this device, or just a reasonable value chosen to detect performance
// regressions. Check the device test configuration for more information.
spi_test!(test_exchange_throughput, device, bus, {
    const BUFFER_SIZE: usize = 65536;
    const TX_VMO_ID: u32 = 1;
    const RX_VMO_ID: u32 = 2;

    let mut txdata = vec![0u8; BUFFER_SIZE];
    rand::rng().fill(&mut txdata[..]);

    let tx_vmo = zx::Vmo::create(BUFFER_SIZE as u64).context("Failed to create TX VMO")?;
    tx_vmo.write(&txdata, 0).context("Failed to write to TX VMO")?;

    let rx_vmo = zx::Vmo::create(BUFFER_SIZE as u64).context("Failed to create RX VMO")?;
    let rx_vmo_dup =
        rx_vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).context("Failed to duplicate RX VMO")?;

    device
        .register_vmo(
            TX_VMO_ID,
            fmem::Range { vmo: tx_vmo, offset: 0, size: BUFFER_SIZE as u64 },
            fsharedmemory::SharedVmoRight::READ,
        )
        .await
        .context("RegisterVmo TX FIDL call failed")?
        .map_err(|status| {
            anyhow::anyhow!("RegisterVmo TX failed: {:?}", Status::err_from_raw(status))
        })?;

    device
        .register_vmo(
            RX_VMO_ID,
            fmem::Range { vmo: rx_vmo_dup, offset: 0, size: BUFFER_SIZE as u64 },
            fsharedmemory::SharedVmoRight::WRITE,
        )
        .await
        .context("RegisterVmo RX FIDL call failed")?
        .map_err(|status| {
            anyhow::anyhow!("RegisterVmo RX failed: {:?}", Status::err_from_raw(status))
        })?;

    let start_time = std::time::Instant::now();

    device
        .exchange(
            &fsharedmemory::SharedVmoBuffer {
                vmo_id: TX_VMO_ID,
                offset: 0,
                size: BUFFER_SIZE as u64,
            },
            &fsharedmemory::SharedVmoBuffer {
                vmo_id: RX_VMO_ID,
                offset: 0,
                size: BUFFER_SIZE as u64,
            },
        )
        .await
        .context("Exchange FIDL call failed")?
        .map_err(|status| anyhow::anyhow!("Exchange failed: {:?}", Status::err_from_raw(status)))?;

    let elapsed = start_time.elapsed();
    let throughput = BUFFER_SIZE as f64 / elapsed.as_secs_f64();

    println!("Elapsed time: {:.2?}, bytes per second: {:.2}", elapsed, throughput);

    anyhow::ensure!(
        throughput >= bus.min_expected_throughput as f64,
        "Throughput {} bytes/s is below minimum expected {} bytes/s",
        throughput,
        bus.min_expected_throughput
    );

    let mut rxdata = vec![0u8; BUFFER_SIZE];
    rx_vmo.read(&mut rxdata, 0).context("Failed to read from RX VMO")?;
    assert_eq!(txdata, rxdata);

    let _unregistered_tx_vmo = device
        .unregister_vmo(TX_VMO_ID)
        .await
        .context("UnregisterVmo TX FIDL call failed")?
        .map_err(|status| {
            anyhow::anyhow!("UnregisterVmo TX failed: {:?}", Status::err_from_raw(status))
        })?;

    let _unregistered_rx_vmo = device
        .unregister_vmo(RX_VMO_ID)
        .await
        .context("UnregisterVmo RX FIDL call failed")?
        .map_err(|status| {
            anyhow::anyhow!("UnregisterVmo RX failed: {:?}", Status::err_from_raw(status))
        })?;

    Ok(())
});

// This test measures the median latency of ten one-byte calls to Exchange() in nanoseconds, which
// is then compared to a maximum expected value for this device. The test passes if the median
// latency is less than or equal to the expected latency. The expected latency may be a hard
// requirement for this device, or just a reasonable value chosen to detect performance regressions.
// Check the device test configuration for more information.
spi_test!(test_latency, device, bus, {
    const BUFFER_SIZE: usize = 1;
    const TX_VMO_ID: u32 = 1;
    const RX_VMO_ID: u32 = 2;

    let mut txdata = vec![0u8; BUFFER_SIZE];
    rand::rng().fill(&mut txdata[..]);

    let tx_vmo = zx::Vmo::create(BUFFER_SIZE as u64).context("Failed to create TX VMO")?;
    tx_vmo.write(&txdata, 0).context("Failed to write to TX VMO")?;

    let rx_vmo = zx::Vmo::create(BUFFER_SIZE as u64).context("Failed to create RX VMO")?;

    device
        .register_vmo(
            TX_VMO_ID,
            fmem::Range { vmo: tx_vmo, offset: 0, size: BUFFER_SIZE as u64 },
            fsharedmemory::SharedVmoRight::READ,
        )
        .await
        .context("RegisterVmo TX FIDL call failed")?
        .map_err(|status| {
            anyhow::anyhow!("RegisterVmo TX failed: {:?}", Status::err_from_raw(status))
        })?;

    device
        .register_vmo(
            RX_VMO_ID,
            fmem::Range { vmo: rx_vmo, offset: 0, size: BUFFER_SIZE as u64 },
            fsharedmemory::SharedVmoRight::WRITE,
        )
        .await
        .context("RegisterVmo RX FIDL call failed")?
        .map_err(|status| {
            anyhow::anyhow!("RegisterVmo RX failed: {:?}", Status::err_from_raw(status))
        })?;

    const TRANSFER_COUNT: usize = 10;
    let mut latencies = Vec::with_capacity(TRANSFER_COUNT);
    for _ in 0..TRANSFER_COUNT {
        let start_time = std::time::Instant::now();

        device
            .exchange(
                &fsharedmemory::SharedVmoBuffer {
                    vmo_id: TX_VMO_ID,
                    offset: 0,
                    size: BUFFER_SIZE as u64,
                },
                &fsharedmemory::SharedVmoBuffer {
                    vmo_id: RX_VMO_ID,
                    offset: 0,
                    size: BUFFER_SIZE as u64,
                },
            )
            .await
            .context("Exchange FIDL call failed")?
            .map_err(|status| {
                anyhow::anyhow!("Exchange failed: {:?}", Status::err_from_raw(status))
            })?;

        latencies.push(start_time.elapsed().as_nanos());
    }

    assert_eq!(latencies.len(), TRANSFER_COUNT);
    latencies.sort();
    let median = (latencies[TRANSFER_COUNT / 2 - 1] + latencies[TRANSFER_COUNT / 2]) / 2;

    println!("Median latency: {} ns", median);

    assert!(
        median <= bus.max_expected_latency as u128,
        "Median latency {} ns is above maximum expected {} ns",
        median,
        bus.max_expected_latency
    );

    let _unregistered_tx_vmo = device
        .unregister_vmo(TX_VMO_ID)
        .await
        .context("UnregisterVmo TX FIDL call failed")?
        .map_err(|status| {
            anyhow::anyhow!("UnregisterVmo TX failed: {:?}", Status::err_from_raw(status))
        })?;

    let _unregistered_rx_vmo = device
        .unregister_vmo(RX_VMO_ID)
        .await
        .context("UnregisterVmo RX FIDL call failed")?
        .map_err(|status| {
            anyhow::anyhow!("UnregisterVmo RX failed: {:?}", Status::err_from_raw(status))
        })?;

    Ok(())
});
