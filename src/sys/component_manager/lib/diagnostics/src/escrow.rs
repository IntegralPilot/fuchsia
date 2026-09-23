// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use errors::ModelError;
use fuchsia_inspect as inspect;
use fuchsia_inspect::{IntExponentialHistogramProperty, IntLinearHistogramProperty};
use fuchsia_sync as fsync;
use inspect::HistogramProperty;
use moniker::Moniker;

use hooks::{Event, EventPayload, EventType, HasEventType, Hook, HooksRegistration};

const STARTED_DURATIONS: &str = "started_durations";
const STOPPED_DURATIONS: &str = "stopped_durations";
const HISTOGRAM: &str = "histogram";

/// [-inf, 4, 7, 10, 16, 28, 52, 100, 196, 388, 772, 1540, 3076, 6148, inf]
const STARTED_DURATIONS_HISTOGRAM_PARAMS: inspect::ExponentialHistogramParams<i64> =
    inspect::ExponentialHistogramParams {
        floor: 4,
        initial_step: 3,
        step_multiplier: 2,
        buckets: 12,
    };

/// [-inf, 10, 20, 30, 40, ..., 240, 250, inf]
const STOPPED_DURATIONS_HISTOGRAM_PARAMS: inspect::LinearHistogramParams<i64> =
    inspect::LinearHistogramParams { floor: 10, step_size: 10, buckets: 24 };

type StopTime = zx::MonotonicInstant;

/// [`DurationStats`] tracks:
///
/// - durations an escrowing component was executing (`started_durations/histogram/MONIKER`)
/// - durations an escrowing component stayed stopped in-between two executions
///   (`stopped_durations/histogram/MONIKER`)
///
/// The tracking begins the first time a component sends an escrow request. Subsequently,
/// started/stopped durations will be tracked regardless if that component keeps sending
/// escrow requests.
///
/// The duration is measured in ticks in the Zircon monotonic clock, hence does
/// not account into times the system is suspended.
pub struct DurationStats {
    // Keeps the inspect node alive.
    _node: inspect::Node,
    started_durations: ComponentHistograms<IntExponentialHistogramProperty>,
    stopped_durations: ComponentHistograms<IntLinearHistogramProperty>,
    // The set of components that have sent an escrow request at least once,
    // and their last stop time.
    escrowing_components: fsync::Mutex<HashMap<Moniker, StopTime>>,
}

impl DurationStats {
    /// Creates a new duration tracker. Data will be written to the given inspect node.
    pub fn new(node: inspect::Node) -> Self {
        let started = node.create_child(STARTED_DURATIONS);
        let histogram = started.create_child(HISTOGRAM);
        node.record(started);
        let started_durations = ComponentHistograms {
            node: histogram,
            properties: Default::default(),
            init: |node, name| {
                node.create_int_exponential_histogram(name, STARTED_DURATIONS_HISTOGRAM_PARAMS)
            },
        };

        let stopped = node.create_child(STOPPED_DURATIONS);
        let histogram = stopped.create_child(HISTOGRAM);
        node.record(stopped);
        let stopped_durations = ComponentHistograms {
            node: histogram,
            properties: Default::default(),
            init: |node, name| {
                node.create_int_linear_histogram(name, STOPPED_DURATIONS_HISTOGRAM_PARAMS)
            },
        };

        Self {
            _node: node,
            started_durations,
            stopped_durations,
            escrowing_components: Default::default(),
        }
    }

    /// Provides the hook events that are needed to work.
    pub fn hooks(self: &Arc<Self>) -> Vec<HooksRegistration> {
        vec![HooksRegistration::new(
            "DurationStats",
            vec![EventType::Started, EventType::Stopped],
            Arc::downgrade(self) as Weak<dyn Hook>,
        )]
    }

    fn on_component_started(self: &Arc<Self>, moniker: &Moniker, start_time: zx::MonotonicInstant) {
        if let Some(stop_time) = self.escrowing_components.lock().get(moniker) {
            let duration = start_time - *stop_time;
            self.stopped_durations.record(moniker, duration.into_seconds());
        }
    }

    fn on_component_stopped(
        self: &Arc<Self>,
        moniker: &Moniker,
        stop_time: zx::MonotonicInstant,
        execution_duration: zx::MonotonicDuration,
        requested_escrow: bool,
    ) {
        let mut escrowing_components = self.escrowing_components.lock();
        if requested_escrow {
            escrowing_components.insert(moniker.clone(), stop_time);
        }
        if !escrowing_components.contains_key(moniker) {
            return;
        }
        self.started_durations.record(moniker, execution_duration.into_seconds());
    }
}

/// Maintains a histogram under each moniker where there is data.
///
/// The histogram will be a child property created under `node`, and will be named using
/// the component's moniker.
struct ComponentHistograms<H: HistogramProperty<Type = i64>> {
    node: inspect::Node,
    properties: fsync::Mutex<HashMap<Moniker, H>>,
    init: fn(&inspect::Node, String) -> H,
}

impl<H: HistogramProperty<Type = i64>> ComponentHistograms<H> {
    fn record(&self, moniker: &Moniker, value: i64) {
        let mut properties = self.properties.lock();

        if let Some(histogram) = properties.get_mut(moniker) {
            histogram.insert(value);
        } else {
            let histogram = (self.init)(&self.node, moniker.to_string());
            histogram.insert(value);

            properties.insert(moniker.clone(), histogram);
        }
    }
}

#[async_trait]
impl Hook for DurationStats {
    async fn on(self: Arc<Self>, event: &Event) -> Result<(), ModelError> {
        let target_moniker = event
            .target_moniker
            .unwrap_instance_moniker_or(ModelError::UnexpectedComponentManagerMoniker)?;
        match event.event_type() {
            EventType::Started => {
                if let EventPayload::Started { runtime, .. } = &event.payload {
                    self.on_component_started(target_moniker, runtime.start_time_monotonic);
                }
            }
            EventType::Stopped => {
                if let EventPayload::Stopped {
                    stop_time_monotonic,
                    execution_duration,
                    requested_escrow,
                    ..
                } = &event.payload
                {
                    self.on_component_stopped(
                        target_moniker,
                        *stop_time_monotonic,
                        *execution_duration,
                        *requested_escrow,
                    );
                }
            }
            _ => {}
        }
        Ok(())
    }
}
