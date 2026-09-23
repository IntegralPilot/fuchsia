// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_hardware_platform_bus as ffhpb;
use futures::lock::Mutex;
use std::collections::HashMap;
use std::rc::Rc;

/// Stats for a specific wake source.
pub struct WakeSourceReport {
    /// The koid that the kernel attributes to this wake source.
    pub koid: zx::Koid,
    /// The optional name for this wake source, as provided by the kernel. The
    /// debug name is not guaranteed to be a string, so we keep it around as
    /// bytes.
    pub debug_name: Option<Vec<u8>>,
    /// The boot timestamp at which this wake source was first signaled, after
    /// either boot, or last read.
    pub initial_signal_time: zx::BootInstant,
    /// The boot timestamp at which this wake source was last signaled, after
    /// `initial_signal_time`
    pub last_signal_time: zx::BootInstant,
    /// The boot timestamp at which this wake source was last acked.
    pub last_ack_time: zx::BootInstant,
    /// The number of time the signal occurred between the initial and last
    /// signal times.
    pub signal_count: usize,
    /// The flags attached to the `koid` by the kernel.
    pub flags: u32,
}

impl std::fmt::Debug for WakeSourceReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let maybe_name = &self.debug_name.as_ref().map(|name| {
            String::from_utf8(name.clone()).unwrap_or_else(|_| "<not a string>".into())
        });
        f.debug_struct("WakeSourceReport")
            .field("koid", &self.koid)
            .field("debug_name", maybe_name)
            .field("initial_signal_time", &self.initial_signal_time)
            .field("last_signal_time", &self.last_signal_time)
            .field("last_ack_time", &self.last_ack_time)
            .field("signal_count", &self.signal_count)
            .field("flags", &self.flags)
            .finish()
    }
}

impl WakeSourceReport {
    pub fn new(koid: zx::Koid) -> Self {
        Self {
            koid,
            debug_name: None,
            initial_signal_time: Default::default(),
            last_signal_time: Default::default(),
            last_ack_time: Default::default(),
            signal_count: 0,
            flags: 0,
        }
    }
}

/// The info about a specific wake source.
#[derive(Debug, Clone)]
struct Info {
    // The wake source's name, as reported by the platform bus.
    name: String,
    // The token related to the wake source. Use this token to query further
    // details about the wake source.
    _token: Rc<Option<zx::Event>>,
}

#[derive(Debug)]
struct Inner {
    info_by_koid: HashMap<zx::Koid, Info>,
}

#[derive(Debug)]
/// Wake source observability manager.
pub struct WakeSourceObservability {
    // Interior-mutable state.
    inner: Mutex<Inner>,
    // The proxy used for resolving the driver name from an interrupt koid.
    platform_bus_proxy: Option<ffhpb::InterruptAttributorProxy>,
    // Used to export current stats.
    sag_event_logger: crate::SagEventLogger,
}

impl WakeSourceObservability {
    pub fn new(
        platform_bus_proxy: Option<ffhpb::InterruptAttributorProxy>,
        sag_event_logger: crate::SagEventLogger,
    ) -> Self {
        let info_by_koid = HashMap::new();
        let inner = Mutex::new(Inner { info_by_koid });
        Self { inner, platform_bus_proxy, sag_event_logger }
    }

    pub async fn register_wake_source_reports(&self, reports: Vec<WakeSourceReport>) {
        for report in reports.iter() {
            log::debug!("power_observability: registered {report:?}");
            let koid = &report.koid;
            // Maybe parallelize.
            let info = self.inner.lock().await.info_by_koid.get(koid).map(|info| info.clone());
            match info {
                Some(old_info) => {
                    log::debug!("wake source: {koid:?}: {old_info:?}");
                }
                None => {
                    if let Some(platform_bus_proxy) = self.platform_bus_proxy.as_ref() {
                        let result = platform_bus_proxy
                            .get_interrupt_info(
                                &ffhpb::InterruptAttributorGetInterruptInfoRequest::InterruptKoid(
                                    koid.raw_koid(),
                                ),
                            )
                            .await;

                        // Maybe obtain other information here.
                        match result {
                            Ok(Ok((name, token))) => {
                                let info = Info { name, _token: Rc::new(token) };
                                self.inner.lock().await.info_by_koid.insert(koid.clone(), info);
                            }
                            Ok(Err(err)) => {
                                log::error!("power_observability: protocol error: {err:?}");
                            }
                            Err(err) => {
                                log::error!("power_observability: FIDL error: {err:?}");
                            }
                        }
                    } else {
                        log::debug!("no interrupt attributor, interrupts will not be resolved.");
                    }
                }
            };
        }

        let reasons = {
            let guard = self.inner.lock().await;
            let info_by_koid = &guard.info_by_koid;

            let result: Vec<String> = reports
                .iter()
                .map(|report| {
                    info_by_koid
                        .get(&report.koid)
                        .map(|info| {
                            let message = format!(
                                "koid:{} name:{} report {report:?}",
                                report.koid.raw_koid(),
                                info.name
                            );
                            log::info!("wake vector: {message}");
                            message
                        })
                        .unwrap_or_else(|| "<unnamed>".into())
                })
                .collect();
            result
        };
        let event = crate::events::SagEvent::WakeReasons { reasons };
        self.sag_event_logger.log(event);
    }
}
