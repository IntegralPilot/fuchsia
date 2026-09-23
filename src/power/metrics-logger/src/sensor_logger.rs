// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::MIN_INTERVAL_FOR_SYSLOG_MS;
use crate::driver_utils::Driver;
use anyhow::{Error, Result, format_err};
use async_trait::async_trait;
use diagnostics_hierarchy::LinearHistogramParams;
use fidl::endpoints::Proxy;
use fidl_fuchsia_hardware_power_sensor as fpower;
use fidl_fuchsia_hardware_temperature as ftemperature;
use fidl_fuchsia_hardware_thermal as fthermal;
use fidl_fuchsia_power_metrics as fmetrics;
use fidl_fuchsia_ui_activity as factivity;
use fuchsia_async as fasync;
use fuchsia_component::client as fclient;
use fuchsia_inspect::{
    self as inspect, ArrayProperty, HistogramProperty, IntLinearHistogramProperty, Property,
};
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use log::{error, info, warn};
use std::cell::Cell;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

// Type aliases for convenience.
pub type TemperatureDriver = Driver<ftemperature::DeviceProxy>;
pub type PowerDriver = Driver<fpower::DeviceProxy>;
pub type TemperatureLogger = SensorLogger<ftemperature::DeviceProxy>;
pub type PowerLogger = SensorLogger<fpower::DeviceProxy>;
pub type TemperatureLoggerArgs<'a> = SensorLoggerArgs<'a, ftemperature::DeviceProxy>;
pub type PowerLoggerArgs<'a> = SensorLoggerArgs<'a, fpower::DeviceProxy>;

const HISTOGRAM_PARAMS: LinearHistogramParams<i64> =
    LinearHistogramParams { floor: 0, step_size: 50, buckets: 100 };

pub async fn generate_temperature_drivers(
    driver_aliases: HashMap<String, String>,
) -> Result<Vec<Driver<ftemperature::DeviceProxy>>> {
    let mut drivers = Vec::new();
    let mut sensor_names: HashSet<String> = HashSet::new();

    for instance in fclient::Service::open(ftemperature::ServiceMarker)?.enumerate().await? {
        let temperature_proxy = instance.connect_to_device()?;
        let sensor_name = temperature_proxy.get_sensor_name().await?;
        if sensor_names.insert(sensor_name.clone()) {
            let alias = driver_aliases.get(&sensor_name);
            drivers.push(Driver { sensor_name, proxy: temperature_proxy, alias: alias.cloned() });
        }
    }

    for instance in fclient::Service::open(fthermal::ServiceMarker)?.enumerate().await? {
        let thermal_proxy = instance.connect_to_device()?;
        // fuchsia.hardware.temperature.Device is composed into fuchsia.hardware.thermal.Device,
        // so we can safely convert the channel and use it as a temperature sensor.
        let channel = thermal_proxy
            .into_channel()
            .map_err(|_| format_err!("Failed to convert thermal proxy to channel"))?;
        let temperature_proxy = ftemperature::DeviceProxy::from_channel(channel);
        let sensor_name = temperature_proxy.get_sensor_name().await?;
        if sensor_names.insert(sensor_name.clone()) {
            let alias = driver_aliases.get(&sensor_name);
            drivers.push(Driver { sensor_name, proxy: temperature_proxy, alias: alias.cloned() });
        }
    }
    Ok(drivers)
}

pub async fn generate_power_drivers(
    driver_aliases: HashMap<String, String>,
) -> Result<Vec<Driver<fpower::DeviceProxy>>> {
    let mut drivers = Vec::new();
    for instance in fclient::Service::open(fpower::ServiceMarker)?.enumerate().await? {
        let power_proxy = instance.connect_to_device()?;
        let sensor_name = power_proxy.get_sensor_name().await?;
        let alias = driver_aliases.get(&sensor_name);
        drivers.push(Driver { sensor_name, proxy: power_proxy, alias: alias.cloned() });
    }
    Ok(drivers)
}

#[derive(Eq, PartialEq)]
pub enum SensorType {
    Temperature,
    Power,
}

#[async_trait(?Send)]
pub trait Sensor<T> {
    fn sensor_type() -> SensorType;
    fn unit() -> String;
    async fn read_data(sensor: &T) -> Result<f32, Error>;

    // TODO(b/322867602): Make units consistent.
    fn unit_for_sampler() -> String;
    fn sampler_multiplier() -> f32;
}

#[async_trait(?Send)]
impl Sensor<ftemperature::DeviceProxy> for ftemperature::DeviceProxy {
    fn sensor_type() -> SensorType {
        SensorType::Temperature
    }

    fn unit() -> String {
        String::from("°C")
    }

    async fn read_data(sensor: &ftemperature::DeviceProxy) -> Result<f32, Error> {
        match sensor.get_temperature_celsius().await {
            Ok((zx_status, temperature)) => match zx::Status::ok(zx_status) {
                Ok(()) => Ok(temperature),
                Err(e) => Err(format_err!("get_temperature_celsius returned an error: {}", e)),
            },
            Err(e) => Err(format_err!("get_temperature_celsius IPC failed: {}", e)),
        }
    }

    fn unit_for_sampler() -> String {
        String::from("°C")
    }

    fn sampler_multiplier() -> f32 {
        1.0
    }
}

#[async_trait(?Send)]
impl Sensor<fpower::DeviceProxy> for fpower::DeviceProxy {
    fn sensor_type() -> SensorType {
        SensorType::Power
    }

    fn unit() -> String {
        String::from("W")
    }

    async fn read_data(sensor: &fpower::DeviceProxy) -> Result<f32, Error> {
        match sensor.get_power_watts().await {
            Ok(result) => match result {
                Ok(power) => Ok(power),
                Err(e) => Err(format_err!("get_power_watts returned an error: {}", e)),
            },
            Err(e) => Err(format_err!("get_power_watts IPC failed: {}", e)),
        }
    }

    // TODO(b/322867602): Make units consistent.
    fn unit_for_sampler() -> String {
        String::from("mW")
    }

    fn sampler_multiplier() -> f32 {
        1000.0
    }
}

macro_rules! log_trace {
    ( $sensor_type:expr, $trace_args:expr) => {
        match $sensor_type {
            SensorType::Temperature => {
                if let Some(context) =
                    fuchsia_trace::TraceCategoryContext::acquire(c"metrics_logger")
                {
                    fuchsia_trace::counter(&context, c"temperature", 0, $trace_args);
                }
            }
            SensorType::Power => {
                if let Some(context) =
                    fuchsia_trace::TraceCategoryContext::acquire(c"metrics_logger")
                {
                    fuchsia_trace::counter(&context, c"power", 0, $trace_args);
                }
            }
        }
    };
}

macro_rules! log_trace_statistics {
    ( $sensor_type:expr, $trace_args:expr) => {
        match $sensor_type {
            SensorType::Temperature => {
                if let Some(context) =
                    fuchsia_trace::TraceCategoryContext::acquire(c"metrics_logger")
                {
                    fuchsia_trace::counter(
                        &context,
                        c"temperature_min",
                        0,
                        &$trace_args[Statistics::Min as usize],
                    );
                    fuchsia_trace::counter(
                        &context,
                        c"temperature_max",
                        0,
                        &$trace_args[Statistics::Max as usize],
                    );
                    fuchsia_trace::counter(
                        &context,
                        c"temperature_avg",
                        0,
                        &$trace_args[Statistics::Avg as usize],
                    );
                    fuchsia_trace::counter(
                        &context,
                        c"temperature_median",
                        0,
                        &$trace_args[Statistics::Median as usize],
                    );
                }
            }
            SensorType::Power => {
                if let Some(context) =
                    fuchsia_trace::TraceCategoryContext::acquire(c"metrics_logger")
                {
                    fuchsia_trace::counter(
                        &context,
                        c"power_min",
                        0,
                        &$trace_args[Statistics::Min as usize],
                    );
                    fuchsia_trace::counter(
                        &context,
                        c"power_max",
                        0,
                        &$trace_args[Statistics::Max as usize],
                    );
                    fuchsia_trace::counter(
                        &context,
                        c"power_avg",
                        0,
                        &$trace_args[Statistics::Avg as usize],
                    );
                    fuchsia_trace::counter(
                        &context,
                        c"power_median",
                        0,
                        &$trace_args[Statistics::Median as usize],
                    );
                }
            }
        }
    };
}

struct StatisticsTracker {
    /// Interval for summarizing statistics.
    statistics_interval: zx::MonotonicDuration,

    /// List of samples polled from all the sensors during `statistics_interval` starting from
    /// `statistics_start_time`. Data is cleared at the end of each `statistics_interval`.
    /// For each sensor, samples are stored in `Vec<f32>` in chronological order.
    samples: Vec<Vec<f32>>,

    /// Start time for a new statistics period.
    /// This is an exclusive start.
    statistics_start_time: fasync::MonotonicInstant,
}

pub struct SensorLoggerArgs<'a, T> {
    /// List of sensor drivers.
    pub drivers: Rc<Vec<Driver<T>>>,

    /// Activity listener to check idleness of the device.
    pub activity_listener: Option<ActivityListener>,

    /// Polling interval from the sensors.
    pub sampling_interval_ms: u32,

    /// Time between statistics calculations.
    pub statistics_interval_ms: Option<u32>,

    /// Amount of time to log sensor data.
    pub duration_ms: Option<u32>,

    /// Root inspect node to post sensor statistics.
    pub client_inspect: &'a inspect::Node,

    /// ID of the client requesting to collect sensor stats.
    pub client_id: String,

    /// Whether to output samples to syslog.
    pub output_samples_to_syslog: bool,

    /// Whether to output computed statistics to syslog.
    pub output_stats_to_syslog: bool,
}

pub struct SensorLogger<T> {
    /// List of sensor drivers.
    drivers: Rc<Vec<Driver<T>>>,

    /// Activity listener to check idleness of the device.
    activity_listener: ActivityListener,

    /// Polling interval from the sensors.
    sampling_interval: zx::MonotonicDuration,

    /// Start time for the logger; used to calculate elapsed time.
    /// This is an exclusive start.
    start_time: fasync::MonotonicInstant,

    /// Time at which the logger will stop.
    /// This is an exclusive end.
    end_time: fasync::MonotonicInstant,

    /// Client associated with this logger.
    client_id: String,

    /// Statistics tracker to collect samples and calculate stats.
    statistics_tracker: Option<StatisticsTracker>,

    /// Inspect data manager.
    inspect: InspectData,

    /// Whether to output samples to syslog.
    output_samples_to_syslog: bool,

    /// Whether to output computed statistics to syslog.
    output_stats_to_syslog: bool,
}

impl<T: Sensor<T>> SensorLogger<T> {
    pub async fn new(args: SensorLoggerArgs<'_, T>) -> Result<Self, fmetrics::RecorderError> {
        let SensorLoggerArgs::<'_, T> {
            drivers,
            activity_listener,
            sampling_interval_ms,
            statistics_interval_ms,
            duration_ms,
            client_inspect,
            client_id,
            output_samples_to_syslog,
            output_stats_to_syslog,
        } = args;

        if let Some(interval) = statistics_interval_ms {
            if sampling_interval_ms > interval
                || duration_ms.is_some_and(|d| d <= interval)
                || output_stats_to_syslog && interval < MIN_INTERVAL_FOR_SYSLOG_MS
            {
                return Err(fmetrics::RecorderError::InvalidStatisticsInterval);
            }
        }
        if sampling_interval_ms == 0
            || output_samples_to_syslog && sampling_interval_ms < MIN_INTERVAL_FOR_SYSLOG_MS
            || duration_ms.is_some_and(|d| d <= sampling_interval_ms)
        {
            return Err(fmetrics::RecorderError::InvalidSamplingInterval);
        }
        if drivers.len() == 0 {
            return Err(fmetrics::RecorderError::NoDrivers);
        }

        let driver_names: Vec<String> = drivers.iter().map(|c| c.name().to_string()).collect();

        let start_time = fasync::MonotonicInstant::now();
        let end_time = duration_ms.map_or(fasync::MonotonicInstant::INFINITE, |d| {
            start_time + zx::MonotonicDuration::from_millis(d as i64)
        });
        let sampling_interval = zx::MonotonicDuration::from_millis(sampling_interval_ms as i64);

        let statistics_tracker = statistics_interval_ms.map(|i| StatisticsTracker {
            statistics_interval: zx::MonotonicDuration::from_millis(i as i64),
            statistics_start_time: fasync::MonotonicInstant::now(),
            samples: vec![Vec::new(); drivers.len()],
        });

        let logger_name = match T::sensor_type() {
            SensorType::Temperature => "TemperatureLogger",
            SensorType::Power => "PowerLogger",
        };
        let inspect = InspectData::new(
            client_inspect,
            logger_name,
            driver_names,
            T::unit(),
            T::unit_for_sampler(),
            T::sampler_multiplier(),
        );

        Ok(SensorLogger {
            drivers,
            sampling_interval,
            start_time,
            end_time,
            client_id,
            statistics_tracker,
            inspect,
            output_samples_to_syslog,
            output_stats_to_syslog,
            activity_listener: activity_listener.unwrap_or_else(|| {
                warn!("No ActivityListener supplied. Running with unknown activity state.");
                ActivityListener::new_disconnected()
            }),
        })
    }

    /// Logs data from all provided sensors.
    pub async fn log_data(mut self) {
        let mut interval = fasync::Interval::new(self.sampling_interval);

        while let Some(()) = interval.next().await {
            // If we're interested in very high-rate polling in the future, it might be worth
            // comparing the elapsed time to the intended polling interval and logging any
            // anomalies.
            let now = fasync::MonotonicInstant::now();
            if now >= self.end_time {
                break;
            }
            self.log_single_data(now).await;
        }
    }

    async fn log_single_data(&mut self, time_stamp: fasync::MonotonicInstant) {
        // Execute a query to each sensor driver.
        let queries = FuturesUnordered::new();
        for (index, driver) in self.drivers.iter().enumerate() {
            let query = async move {
                let result = T::read_data(&driver.proxy).await;
                (index, result)
            };

            queries.push(query);
        }
        let results = queries.collect::<Vec<(usize, Result<f32, Error>)>>().await;

        // Current statistics interval is (self.statistics_start_time,
        // self.statistics_start_time + self.statistics_interval]. Check if current sample
        // is the last sample of the current statistics interval.
        let is_last_sample_for_statistics = self
            .statistics_tracker
            .as_ref()
            .is_some_and(|t| time_stamp - t.statistics_start_time >= t.statistics_interval);

        let mut trace_args = Vec::new();
        let mut trace_args_statistics = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];

        let mut sensor_names = Vec::new();
        for driver in self.drivers.iter() {
            let sensor_name = &driver.sensor_name;
            let name = driver.alias.as_ref().map_or_else(
                || sensor_name.to_string(),
                |alias| format!("{}(alias:{})", sensor_name, alias),
            );
            sensor_names.push(name);
        }

        for (index, result) in results.into_iter() {
            match result {
                Ok(value) => {
                    // Save the current sample for calculating statistics.
                    if let Some(tracker) = &mut self.statistics_tracker {
                        tracker.samples[index].push(value);
                    }

                    // Log data to Inspect.
                    self.inspect.log_data(
                        index,
                        value,
                        (time_stamp - self.start_time).into_millis(),
                    );

                    trace_args.push(fuchsia_trace::ArgValue::of(
                        self.drivers[index].name(),
                        value as f64,
                    ));

                    if self.output_samples_to_syslog {
                        info!(
                            name = sensor_names[index].as_str(),
                            unit = T::unit().as_str(),
                            value;
                            "Reading sensor"
                        );
                    }
                }
                // In case of a polling error, the previous value from this sensor will not be
                // updated. We could do something fancier like exposing an error count, but this
                // sample will be missing from the trace counter as is, and any serious analysis
                // should be performed on the trace. This sample will also be missing for
                // calculating statistics.
                Err(err) => error!(
                    err:?,
                    path = self.drivers[index].sensor_name.as_str();
                    "Error reading sensor",
                ),
            };

            if is_last_sample_for_statistics {
                if let Some(tracker) = &mut self.statistics_tracker {
                    let mut min = f32::MAX;
                    let mut max = f32::MIN;
                    let mut sum: f32 = 0.0;
                    for sample in &tracker.samples[index] {
                        min = f32::min(min, *sample);
                        max = f32::max(max, *sample);
                        sum += *sample;
                    }
                    let avg = sum / tracker.samples[index].len() as f32;

                    let mut samples = tracker.samples[index].clone();
                    // f32 doesn't support Ord, so can't use samples.sort().
                    samples.sort_by(|x, y| x.partial_cmp(y).unwrap_or(Ordering::Less));
                    let med = samples[samples.len() / 2];

                    self.inspect.log_statistics(
                        index,
                        (tracker.statistics_start_time - self.start_time).into_millis(),
                        (time_stamp - self.start_time).into_millis(),
                        min,
                        max,
                        avg,
                        med,
                        self.activity_listener.idleness.get(),
                    );

                    trace_args_statistics[Statistics::Min as usize]
                        .push(fuchsia_trace::ArgValue::of(&sensor_names[index], min as f64));
                    trace_args_statistics[Statistics::Max as usize]
                        .push(fuchsia_trace::ArgValue::of(&sensor_names[index], max as f64));
                    trace_args_statistics[Statistics::Avg as usize]
                        .push(fuchsia_trace::ArgValue::of(&sensor_names[index], avg as f64));
                    trace_args_statistics[Statistics::Median as usize]
                        .push(fuchsia_trace::ArgValue::of(&sensor_names[index], med as f64));

                    if self.output_stats_to_syslog {
                        info!(
                            name = sensor_names[index].as_str(),
                            max,
                            min,
                            avg,
                            med,
                            unit = T::unit().as_str();
                            "Sensor statistics",
                        );
                    }

                    // Empty samples for this sensor.
                    tracker.samples[index].clear();
                }
            }
        }

        trace_args.push(fuchsia_trace::ArgValue::of("client_id", self.client_id.as_str()));
        log_trace!(T::sensor_type(), &trace_args);

        if is_last_sample_for_statistics {
            for t in trace_args_statistics.iter_mut() {
                t.push(fuchsia_trace::ArgValue::of("client_id", self.client_id.as_str()));
            }
            log_trace_statistics!(T::sensor_type(), trace_args_statistics);

            // Reset timestamp to the calculated theoretical start time of next cycle.
            self.statistics_tracker
                .as_mut()
                .map(|t| t.statistics_start_time += t.statistics_interval);
        }
    }
}

#[derive(Debug)]
pub struct ActivityListener {
    idleness: Rc<Cell<Idleness>>,
}

impl ActivityListener {
    pub fn new_disconnected() -> Self {
        Self { idleness: Rc::new(Cell::new(Idleness::Unknown)) }
    }

    pub fn new(provider: factivity::ProviderProxy) -> Result<Self> {
        let idleness = Rc::new(Cell::new(Idleness::Unknown));
        let (client_end, mut listener_stream) =
            fidl::endpoints::create_request_stream::<factivity::ListenerMarker>();
        provider.watch_state(client_end)?;

        let self_idleness = idleness.clone();
        fasync::Task::local(async move {
            while let Some(Ok(event)) = listener_stream.next().await {
                match event {
                    factivity::ListenerRequest::OnStateChanged {
                        state,
                        transition_time: _,
                        responder,
                    } => {
                        let new_idleness = match state {
                            factivity::State::Unknown => Idleness::Unknown,
                            factivity::State::Idle => Idleness::Idle,
                            factivity::State::Active => Idleness::Active,
                        };

                        self_idleness.set(new_idleness);

                        let _ = responder.send();
                    }
                }
            }
        })
        .detach();

        Ok(Self { idleness })
    }
}

enum Statistics {
    Min = 0,
    Max,
    Avg,
    Median,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Idleness {
    Unknown = 0,
    Idle,
    Active,
}

impl Idleness {
    pub fn as_str(&self) -> &str {
        match self {
            Idleness::Unknown => "unknown",
            Idleness::Idle => "idle",
            Idleness::Active => "active",
        }
    }
}

struct InspectData {
    data: Vec<inspect::DoubleProperty>,
    histograms_by_idleness: HashMap<Idleness, Vec<Vec<IntLinearHistogramProperty>>>,
    statistics: Vec<Vec<inspect::DoubleProperty>>,
    statistics_periods: Vec<inspect::IntArrayProperty>,
    elapsed_millis: Option<inspect::IntProperty>,
    sensor_nodes: Vec<inspect::Node>,
    logger_root: inspect::Node,
    sensor_names: Vec<String>,
    unit: String,
    unit_for_sampler: String,
    sampler_multiplier: f32,
}

impl InspectData {
    fn new(
        parent: &inspect::Node,
        logger_name: &str,
        sensor_names: Vec<String>,
        unit: String,
        unit_for_sampler: String,
        sampler_multiplier: f32,
    ) -> Self {
        Self {
            logger_root: parent.create_child(logger_name),
            data: Vec::new(),
            histograms_by_idleness: HashMap::new(),
            statistics: Vec::new(),
            statistics_periods: Vec::new(),
            elapsed_millis: None,
            sensor_nodes: Vec::new(),
            sensor_names,
            unit,
            unit_for_sampler,
            sampler_multiplier,
        }
    }

    fn init_nodes_for_logging_data(&mut self) {
        self.logger_root.atomic_update(|logger_root| {
            self.elapsed_millis = Some(logger_root.create_int("elapsed time (ms)", i64::MIN));
            self.sensor_nodes =
                self.sensor_names.iter().map(|name| logger_root.create_child(name)).collect();
            for node in self.sensor_nodes.iter() {
                self.data.push(node.create_double(format!("data ({})", self.unit), f64::MIN));
            }
        });
    }

    fn init_nodes_for_logging_stats(&mut self) {
        for node in self.sensor_nodes.iter() {
            node.atomic_update(|node| {
                node.record_child("histograms", |histograms_node| {
                    histograms_node.record_string("unit", &self.unit_for_sampler);
                    for idleness in [Idleness::Unknown, Idleness::Idle, Idleness::Active] {
                        histograms_node.record_child(idleness.as_str(), |n| {
                            self.histograms_by_idleness.entry(idleness).or_default().push(vec![
                                n.create_int_linear_histogram("min", HISTOGRAM_PARAMS.clone()),
                                n.create_int_linear_histogram("max", HISTOGRAM_PARAMS.clone()),
                                n.create_int_linear_histogram("average", HISTOGRAM_PARAMS.clone()),
                                n.create_int_linear_histogram("median", HISTOGRAM_PARAMS.clone()),
                            ]);
                        });
                    }
                });

                node.record_child("statistics", |statistics_node| {
                    let statistics_period =
                        statistics_node.create_int_array("(start ms, end ms]", 2);
                    statistics_period.set(0, i64::MIN);
                    statistics_period.set(1, i64::MIN);
                    self.statistics_periods.push(statistics_period);

                    // The indices of the statistics child nodes match the sequence defined in
                    // `Statistics`.
                    self.statistics.push(vec![
                        statistics_node.create_double(format!("min ({})", self.unit), f64::MIN),
                        statistics_node.create_double(format!("max ({})", self.unit), f64::MIN),
                        statistics_node.create_double(format!("average ({})", self.unit), f64::MIN),
                        statistics_node.create_double(format!("median ({})", self.unit), f64::MIN),
                    ]);
                });
            });
        }
    }

    fn log_data(&mut self, index: usize, value: f32, elapsed_millis: i64) {
        if self.data.is_empty() {
            self.init_nodes_for_logging_data();
        }
        self.elapsed_millis.as_ref().map(|e| e.set(elapsed_millis));
        self.data[index].set(value as f64);
    }

    fn log_statistics(
        &mut self,
        index: usize,
        start_time: i64,
        end_time: i64,
        min: f32,
        max: f32,
        avg: f32,
        med: f32,
        idleness: Idleness,
    ) {
        if self.statistics.is_empty() {
            self.init_nodes_for_logging_stats();
        }

        self.statistics_periods[index].set(0, start_time);
        self.statistics_periods[index].set(1, end_time);

        self.statistics[index][Statistics::Min as usize].set(min as f64);
        self.statistics[index][Statistics::Max as usize].set(max as f64);
        self.statistics[index][Statistics::Avg as usize].set(avg as f64);
        self.statistics[index][Statistics::Median as usize].set(med as f64);

        let histograms =
            self.histograms_by_idleness.get(&idleness).expect("histograms not initialized");
        histograms[index][Statistics::Min as usize].insert((min * self.sampler_multiplier) as i64);
        histograms[index][Statistics::Max as usize].insert((max * self.sampler_multiplier) as i64);
        histograms[index][Statistics::Avg as usize].insert((avg * self.sampler_multiplier) as i64);
        histograms[index][Statistics::Median as usize]
            .insert((med * self.sampler_multiplier) as i64);
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use diagnostics_assertions::{HistogramAssertion, assert_data_tree};
    use futures::task::Poll;
    use futures::{FutureExt, TryStreamExt};
    use std::cell::OnceCell;
    use std::pin::Pin;

    fn setup_fake_temperature_driver(
        mut get_temperature: impl FnMut() -> f32 + 'static,
    ) -> (ftemperature::DeviceProxy, fasync::Task<()>) {
        let (proxy, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<ftemperature::DeviceMarker>();
        let task = fasync::Task::local(async move {
            while let Ok(Some(req)) = stream.try_next().await {
                match req {
                    ftemperature::DeviceRequest::GetTemperatureCelsius { responder } => {
                        let _ = responder.send(zx::sys::ZX_OK, get_temperature());
                    }
                    _ => panic!("Unexpected request"),
                }
            }
        });

        (proxy, task)
    }

    fn setup_fake_power_driver(
        mut get_power: impl FnMut() -> f32 + 'static,
    ) -> (fpower::DeviceProxy, fasync::Task<()>) {
        let (proxy, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<fpower::DeviceMarker>();
        let task = fasync::Task::local(async move {
            while let Ok(Some(req)) = stream.try_next().await {
                match req {
                    fpower::DeviceRequest::GetPowerWatts { responder } => {
                        let _ = responder.send(Ok(get_power()));
                    }
                    _ => panic!("Unexpected request"),
                }
            }
        });

        (proxy, task)
    }

    // Convenience function to create a vector of two temperature drivers for test usage.
    // Returns a tuple of:
    // - Vec<fasync::Task<()>>: Tasks for handling driver query stream.
    // - Vec<TemperatureDriver>: Fake temperature drivers for test usage.
    // - Rc<Cell<f32>>: Pointer for setting fake temperature data in the first driver.
    // - Rc<Cell<f32>>> Pointer for setting fake temperature data in the second driver.
    pub fn create_temperature_drivers()
    -> (Vec<fasync::Task<()>>, Vec<TemperatureDriver>, Rc<Cell<f32>>, Rc<Cell<f32>>) {
        let mut tasks = Vec::new();
        let cpu_temperature = Rc::new(Cell::new(0.0));
        let cpu_temperature_clone = cpu_temperature.clone();
        let (cpu_temperature_proxy, task) =
            setup_fake_temperature_driver(move || cpu_temperature_clone.get());
        tasks.push(task);
        let gpu_temperature = Rc::new(Cell::new(0.0));
        let gpu_temperature_clone = gpu_temperature.clone();
        let (gpu_temperature_proxy, task) =
            setup_fake_temperature_driver(move || gpu_temperature_clone.get());
        tasks.push(task);

        let temperature_drivers = vec![
            TemperatureDriver {
                alias: None,
                sensor_name: "cpu".to_string(),
                proxy: cpu_temperature_proxy,
            },
            TemperatureDriver {
                alias: Some("audio_alias".to_string()),
                sensor_name: "audio".to_string(),
                proxy: gpu_temperature_proxy,
            },
        ];

        (tasks, temperature_drivers, cpu_temperature, gpu_temperature)
    }

    // Convenience function to create a vector of two power drivers for test usage.
    // Returns a tuple of:
    // - Vec<fasync::Task<()>>: Tasks for handling driver query stream.
    // - Vec<PowerDriver>: Fake power drivers for test usage.
    // - Rc<Cell<f32>>: Pointer for setting fake power data in the first driver.
    // - Rc<Cell<f32>>> Pointer for setting fake power data in the second driver.
    pub fn create_power_drivers_with_getters(
        get_power1: impl FnMut() -> f32 + 'static,
        get_power2: impl FnMut() -> f32 + 'static,
    ) -> (Vec<fasync::Task<()>>, Vec<PowerDriver>) {
        let mut tasks = Vec::new();
        let (power_1_proxy, task) = setup_fake_power_driver(get_power1);
        tasks.push(task);
        let (power_2_proxy, task) = setup_fake_power_driver(get_power2);
        tasks.push(task);

        let power_drivers = vec![
            PowerDriver { alias: None, sensor_name: "power_1".to_string(), proxy: power_1_proxy },
            PowerDriver {
                alias: Some("power_alias".to_string()),
                sensor_name: "power_2".to_string(),
                proxy: power_2_proxy,
            },
        ];
        (tasks, power_drivers)
    }

    // Convenience function to create a vector of two power drivers for test usage.
    // Returns a tuple of:
    // - Vec<fasync::Task<()>>: Tasks for handling driver query stream.
    // - Vec<PowerDriver>: Fake power drivers for test usage.
    // - Rc<Cell<f32>>: Pointer for setting fake power data in the first driver.
    // - Rc<Cell<f32>>> Pointer for setting fake power data in the second driver.
    pub fn create_power_drivers()
    -> (Vec<fasync::Task<()>>, Vec<PowerDriver>, Rc<Cell<f32>>, Rc<Cell<f32>>) {
        let power_1 = Rc::new(Cell::new(0.0));
        let power_1_clone = power_1.clone();
        let power_2 = Rc::new(Cell::new(0.0));
        let power_2_clone = power_2.clone();
        let (tasks, power_drivers) = create_power_drivers_with_getters(
            move || power_1_clone.get(),
            move || power_2_clone.get(),
        );
        (tasks, power_drivers, power_1, power_2)
    }

    struct Runner {
        inspector: inspect::Inspector,
        inspect_root: inspect::Node,

        _tasks: Vec<fasync::Task<()>>,

        cpu_temperature: Rc<Cell<f32>>,
        gpu_temperature: Rc<Cell<f32>>,
        temperature_drivers: Rc<Vec<TemperatureDriver>>,

        power_1: Rc<Cell<f32>>,
        power_2: Rc<Cell<f32>>,
        power_drivers: Rc<Vec<PowerDriver>>,

        // Fields are dropped in declaration order. Always drop executor last because we hold other
        // zircon objects tied to the executor in this struct, and those can't outlive the executor.
        //
        // See
        // - https://fuchsia-docs.firebaseapp.com/rust/fuchsia_async/struct.TestExecutor.html
        // - https://doc.rust-lang.org/reference/destructors.html.
        executor: fasync::TestExecutor,
    }

    impl Runner {
        fn new() -> Self {
            let executor = fasync::TestExecutor::new_with_fake_time();
            executor.set_fake_time(fasync::MonotonicInstant::from_nanos(0));

            let inspector = inspect::Inspector::default();
            let inspect_root = inspector.root().create_child("MetricsLogger");

            let mut tasks = vec![];
            let (temperature_tasks, drivers, cpu_temperature, gpu_temperature) =
                create_temperature_drivers();
            let temperature_drivers = Rc::new(drivers);
            tasks.extend(temperature_tasks);
            let (power_tasks, drivers, power_1, power_2) = create_power_drivers();
            let power_drivers = Rc::new(drivers);
            tasks.extend(power_tasks);

            Self {
                executor,
                inspector,
                inspect_root,
                cpu_temperature,
                gpu_temperature,
                temperature_drivers,
                power_1,
                power_2,
                power_drivers,
                _tasks: tasks,
            }
        }

        fn iterate_task(&mut self, task: &mut Pin<Box<dyn futures::Future<Output = ()>>>) -> bool {
            let Some(next_time) = fasync::TestExecutor::next_timer() else { return false };
            self.executor.set_fake_time(next_time);
            let _ = self.executor.run_until_stalled(task);
            true
        }
    }

    #[fuchsia::test]
    fn test_logging_temperature_to_inspect() {
        let mut runner = Runner::new();

        let client_id = "test".to_string();
        let client_inspect = runner.inspect_root.create_child(&client_id);
        let poll = runner.executor.run_until_stalled(
            &mut TemperatureLogger::new(TemperatureLoggerArgs {
                drivers: runner.temperature_drivers.clone(),
                activity_listener: None,
                sampling_interval_ms: 100,
                statistics_interval_ms: None,
                duration_ms: Some(1_000),
                client_inspect: &client_inspect,
                client_id: client_id,
                output_samples_to_syslog: false,
                output_stats_to_syslog: false,
            })
            .boxed_local(),
        );

        let temperature_logger = match poll {
            Poll::Ready(Ok(temperature_logger)) => temperature_logger,
            _ => panic!("Failed to create TemperatureLogger"),
        };
        let mut task = temperature_logger.log_data().boxed_local();
        assert_matches!(runner.executor.run_until_stalled(&mut task), Poll::Pending);

        // Check TemperatureLogger added before first temperature poll.
        assert_data_tree!(
            @executor runner.executor,
            runner.inspector,
            root: {
                MetricsLogger: {
                    test: {
                        TemperatureLogger: {
                        }
                    }
                }
            }
        );

        // For the first 9 steps, CPU and GPU temperature are logged to Insepct.
        for i in 0..9 {
            runner.cpu_temperature.set(30.0 + i as f32);
            runner.gpu_temperature.set(40.0 + i as f32);
            runner.iterate_task(&mut task);
            assert_data_tree!(
                @executor runner.executor,
                runner.inspector,
                root: {
                    MetricsLogger: {
                        test: {
                            TemperatureLogger: {
                                "elapsed time (ms)": 100 * (1 + i as i64),
                                "cpu": {
                                    "data (°C)": runner.cpu_temperature.get() as f64,
                                },
                                "audio_alias": {
                                    "data (°C)": runner.gpu_temperature.get() as f64,
                                }
                            }
                        }
                    }
                }
            );
        }

        // With one more time step, the end time has been reached.
        runner.iterate_task(&mut task);
        assert_data_tree!(
            @executor runner.executor,
            runner.inspector,
            root: {
                MetricsLogger: {
                    test: {
                    }
                }
            }
        );
    }

    #[fuchsia::test]
    fn test_logging_power_to_inspect() {
        let mut runner = Runner::new();

        runner.power_1.set(2.0);
        runner.power_2.set(5.0);

        let client_id = "test".to_string();
        let client_inspect = runner.inspect_root.create_child(&client_id);
        let poll = runner.executor.run_until_stalled(
            &mut PowerLogger::new(PowerLoggerArgs {
                drivers: runner.power_drivers.clone(),
                activity_listener: None,
                sampling_interval_ms: 100,
                statistics_interval_ms: Some(100),
                duration_ms: Some(200),
                client_inspect: &client_inspect,
                client_id: client_id,
                output_samples_to_syslog: false,
                output_stats_to_syslog: false,
            })
            .boxed_local(),
        );

        let power_logger = match poll {
            Poll::Ready(Ok(power_logger)) => power_logger,
            _ => panic!("Failed to create PowerLogger"),
        };
        let mut task = power_logger.log_data().boxed_local();
        assert_matches!(runner.executor.run_until_stalled(&mut task), Poll::Pending);

        // Check PowerLogger added before first power sensor poll.
        assert_data_tree!(
            @executor runner.executor,
            runner.inspector,
            root: {
                MetricsLogger: {
                    test: {
                        PowerLogger: {
                        }
                    }
                }
            }
        );

        // Run 1 logging task.
        assert!(runner.iterate_task(&mut task));
        assert_data_tree!(
            @executor runner.executor,
            runner.inspector,
            root: {
                MetricsLogger: {
                    test: {
                        PowerLogger: {
                            "elapsed time (ms)": 100i64,
                            "power_1": contains {
                                "data (W)":2.0,
                                "statistics": {
                                    "(start ms, end ms]": vec![0i64, 100i64],
                                    "max (W)": 2.0,
                                    "min (W)": 2.0,
                                    "average (W)": 2.0,
                                    "median (W)": 2.0,
                                }
                            },
                            "power_alias": contains {
                                "data (W)": 5.0,
                                "statistics": {
                                    "(start ms, end ms]": vec![0i64, 100i64],
                                    "max (W)": 5.0,
                                    "min (W)": 5.0,
                                    "average (W)": 5.0,
                                    "median (W)": 5.0,
                                }
                            }
                        }
                    }
                }
            }
        );

        // Finish the remaining task.
        assert!(runner.iterate_task(&mut task));
        assert_data_tree!(
            @executor runner.executor,
            runner.inspector,
            root: {
                MetricsLogger: {
                    test: {
                    }
                }
            }
        );
    }

    #[fuchsia::test]
    fn test_logging_statistics_to_inspect() {
        let mut runner = Runner::new();

        let client_id = "test".to_string();
        let client_inspect = runner.inspect_root.create_child(&client_id);
        let poll = runner.executor.run_until_stalled(
            &mut TemperatureLogger::new(TemperatureLoggerArgs {
                drivers: runner.temperature_drivers.clone(),
                activity_listener: None,
                sampling_interval_ms: 100,
                statistics_interval_ms: Some(300),
                duration_ms: Some(1_000),
                client_inspect: &client_inspect,
                client_id: client_id,
                output_samples_to_syslog: false,
                output_stats_to_syslog: false,
            })
            .boxed_local(),
        );

        let temperature_logger = match poll {
            Poll::Ready(Ok(temperature_logger)) => temperature_logger,
            _ => panic!("Failed to create TemperatureLogger"),
        };
        let mut task = temperature_logger.log_data().boxed_local();
        assert_matches!(runner.executor.run_until_stalled(&mut task), Poll::Pending);

        for i in 0..9 {
            runner.cpu_temperature.set(30.0 + i as f32);
            runner.gpu_temperature.set(40.0 + i as f32);
            runner.iterate_task(&mut task);

            if i < 2 {
                // Check statistics data is not available for the first 200 ms.
                assert_data_tree!(
                    @executor runner.executor,
                    runner.inspector,
                    root: {
                        MetricsLogger: {
                            test: {
                                TemperatureLogger: {
                                    "elapsed time (ms)": 100 * (1 + i as i64),
                                    "cpu": {
                                        "data (°C)": runner.cpu_temperature.get() as f64,
                                    },
                                    "audio_alias": {
                                        "data (°C)": runner.gpu_temperature.get() as f64,
                                    }
                                }
                            }
                        }
                    }
                );
            } else {
                // Check statistics data is updated every 300 ms.
                assert_data_tree!(
                    @executor runner.executor,
                    runner.inspector,
                    root: {
                        MetricsLogger: {
                            test: {
                                TemperatureLogger: {
                                    "elapsed time (ms)": 100 * (i + 1 as i64),
                                    "cpu": contains {
                                        "data (°C)": (30 + i) as f64,
                                        "statistics": {
                                            "(start ms, end ms]":
                                                vec![100 * (i - 2 - (i + 1) % 3 as i64),
                                                     100 * (i + 1 - (i + 1) % 3 as i64)],
                                            "max (°C)": (30 + i - (i + 1) % 3) as f64,
                                            "min (°C)": (28 + i - (i + 1) % 3) as f64,
                                            "average (°C)": (29 + i - (i + 1) % 3) as f64,
                                            "median (°C)": (29 + i - (i + 1) % 3) as f64,
                                        }
                                    },
                                    "audio_alias": contains {
                                        "data (°C)": (40 + i) as f64,
                                        "statistics": {
                                            "(start ms, end ms]":
                                                vec![100 * (i - 2 - (i + 1) % 3 as i64),
                                                     100 * (i + 1 - (i + 1) % 3 as i64)],
                                            "max (°C)": (40 + i - (i + 1) % 3) as f64,
                                            "min (°C)": (38 + i - (i + 1) % 3) as f64,
                                            "average (°C)": (39 + i - (i + 1) % 3) as f64,
                                            "median (°C)": (39 + i - (i + 1) % 3) as f64,
                                        }
                                    }
                                }
                            }
                        }
                    }
                );
            }
        }

        // With one more time step, the end time has been reached.
        runner.iterate_task(&mut task);
        assert_data_tree!(
            @executor runner.executor,
            runner.inspector,
            root: {
                MetricsLogger: {
                    test: {
                    }
                }
            }
        );
    }

    #[fuchsia::test]
    #[ignore] // TODO(https://b/369178320): Fix this test
    fn test_logging_power_to_inspect_updates_histograms() {
        let mut runner = Runner::new();

        runner.power_1.set(3.0);
        runner.power_2.set(7.0);

        let (provider_proxy, mut provider_stream) =
            fidl::endpoints::create_proxy_and_stream::<factivity::ProviderMarker>();
        let listener_proxy: Rc<OnceCell<factivity::ListenerProxy>> = Rc::new(OnceCell::new());
        let listener_proxy2 = listener_proxy.clone();

        fasync::Task::local(async move {
            while let Some(Ok(req)) = provider_stream.next().await {
                match req {
                    factivity::ProviderRequest::WatchState { listener, .. } => {
                        listener_proxy2.set(listener.into_proxy()).unwrap();
                    }
                }
            }
        })
        .detach();

        let client_id = "test".to_string();
        let client_inspect = runner.inspect_root.create_child(&client_id);
        let poll = runner.executor.run_until_stalled(
            &mut PowerLogger::new(PowerLoggerArgs {
                drivers: runner.power_drivers.clone(),
                activity_listener: ActivityListener::new(provider_proxy).ok(),
                sampling_interval_ms: 100,
                statistics_interval_ms: Some(100),
                duration_ms: Some(400),
                client_inspect: &client_inspect,
                client_id: client_id,
                output_samples_to_syslog: false,
                output_stats_to_syslog: false,
            })
            .boxed_local(),
        );

        let power_logger = match poll {
            Poll::Ready(Ok(power_logger)) => power_logger,
            _ => panic!("Failed to create PowerLogger"),
        };
        let mut task = power_logger.log_data().boxed_local();
        assert_matches!(runner.executor.run_until_stalled(&mut task), Poll::Pending);

        // Check PowerLogger added before first power sensor poll.
        assert_data_tree!(
            @executor runner.executor,
            runner.inspector,
            root: {
                MetricsLogger: {
                    test: {
                        PowerLogger: {
                        }
                    }
                }
            }
        );

        // Run 1 logging task.
        assert!(runner.iterate_task(&mut task));

        fn histogram(values: impl IntoIterator<Item = i64>) -> HistogramAssertion<i64> {
            let mut h = HistogramAssertion::linear(LinearHistogramParams::<i64> {
                floor: 0,
                step_size: 50,
                buckets: 100,
            });
            h.insert_values(values);
            h
        }

        assert_data_tree!(
            @executor runner.executor,
            runner.inspector,
            root: {
                MetricsLogger: {
                    test: {
                        PowerLogger: {
                            "elapsed time (ms)": 100i64,
                            "power_1": contains {
                                "histograms": {
                                    "unit": "mW",
                                    "active": {
                                        "min": histogram([]),
                                        "max": histogram([]),
                                        "average": histogram([]),
                                        "median": histogram([]),
                                    },
                                    "unknown": {
                                        "min": histogram([3000i64]),
                                        "max": histogram([3000i64]),
                                        "average": histogram([3000i64]),
                                        "median": histogram([3000i64]),
                                    },
                                    "idle": {
                                        "min": histogram([]),
                                        "max": histogram([]),
                                        "average": histogram([]),
                                        "median": histogram([]),
                                    }
                                }
                            },
                            "power_alias": contains {
                                "histograms": {
                                    "unit": "mW",
                                    "active": {
                                        "min": histogram([]),
                                        "max": histogram([]),
                                        "average": histogram([]),
                                        "median": histogram([]),
                                    },
                                    "unknown": {
                                        "min": histogram([7000i64]),
                                        "max": histogram([7000i64]),
                                        "average": histogram([7000i64]),
                                        "median": histogram([7000i64]),
                                    },
                                    "idle": {
                                        "min": histogram([]),
                                        "max": histogram([]),
                                        "average": histogram([]),
                                        "median": histogram([]),
                                    }
                                }
                            }
                        }
                    }
                }
            }
        );

        assert!(
            runner.iterate_task(
                &mut listener_proxy
                    .get()
                    .unwrap()
                    .on_state_changed(factivity::State::Idle, 101i64)
                    .map(|f| f.unwrap())
                    .boxed_local(),
            )
        );
        runner.power_1.set(3.2);
        runner.power_2.set(7.2);

        // Run 1 logging task.
        assert!(runner.iterate_task(&mut task));
        assert_data_tree!(
            @executor runner.executor,
            runner.inspector,
            root: {
                MetricsLogger: {
                    test: {
                        PowerLogger: {
                            "elapsed time (ms)": 200i64,
                            "power_1": contains {
                                "histograms": {
                                    "unit": "mW",
                                    "idle": {
                                        "min": histogram([3200i64]),
                                        "max": histogram([3200i64]),
                                        "average": histogram([3200i64]),
                                        "median": histogram([3200i64]),

                                    },
                                    "unknown": {
                                        "min": histogram([3000i64]),
                                        "max": histogram([3000i64]),
                                        "average": histogram([3000i64]),
                                        "median": histogram([3000i64]),

                                    },
                                    "active": {
                                        "min": histogram([]),
                                        "max": histogram([]),
                                        "average": histogram([]),
                                        "median": histogram([]),
                                    }
                                }
                            },
                            "power_alias": contains {
                                "histograms": {
                                    "unit": "mW",
                                    "idle": {
                                        "min": histogram([7200i64]),
                                        "max": histogram([7200i64]),
                                        "average": histogram([7200i64]),
                                        "median": histogram([7200i64]),
                                    },
                                    "unknown": {
                                        "min": histogram([7000i64]),
                                        "max": histogram([7000i64]),
                                        "average": histogram([7000i64]),
                                        "median": histogram([7000i64]),

                                    },
                                    "active": {
                                        "min": histogram([]),
                                        "max": histogram([]),
                                        "average": histogram([]),
                                        "median": histogram([]),
                                    }
                                }
                            }
                        }
                    }
                }
            }
        );

        assert!(
            runner.iterate_task(
                &mut listener_proxy
                    .get()
                    .unwrap()
                    .on_state_changed(factivity::State::Active, 201i64)
                    .map(|f| f.unwrap())
                    .boxed_local(),
            )
        );
        runner.power_1.set(3.5);
        runner.power_2.set(7.5);

        // Run 1 logging task.
        assert!(runner.iterate_task(&mut task));
        assert_data_tree!(
            @executor runner.executor,
            runner.inspector,
            root: {
                MetricsLogger: {
                    test: {
                        PowerLogger: {
                            "elapsed time (ms)": 300i64,
                            "power_1": contains {
                                "histograms": {
                                    "unit": "mW",
                                    "active": {
                                        "min": histogram([3500i64]),
                                        "max": histogram([3500i64]),
                                        "average": histogram([3500i64]),
                                        "median": histogram([3500i64]),
                                    },
                                    "unknown": {
                                        "min": histogram([3000i64]),
                                        "max": histogram([3000i64]),
                                        "average": histogram([3000i64]),
                                        "median": histogram([3000i64]),
                                    },
                                    "idle": {
                                        "min": histogram([3200i64]),
                                        "max": histogram([3200i64]),
                                        "average": histogram([3200i64]),
                                        "median": histogram([3200i64]),
                                    }
                                }
                            },
                            "power_alias": contains {
                                "histograms": {
                                    "unit": "mW",
                                    "active": {
                                        "min": histogram([7500i64]),
                                        "max": histogram([7500i64]),
                                        "average": histogram([7500i64]),
                                        "median": histogram([7500i64]),
                                    },
                                    "unknown": {
                                        "min": histogram([7000i64]),
                                        "max": histogram([7000i64]),
                                        "average": histogram([7000i64]),
                                        "median": histogram([7000i64]),
                                    },
                                    "idle": {
                                        "min": histogram([7200i64]),
                                        "max": histogram([7200i64]),
                                        "average": histogram([7200i64]),
                                        "median": histogram([7200i64]),
                                    }
                                }
                            }
                        }
                    }
                }
            }
        );

        // Finish the remaining task.
        assert!(runner.iterate_task(&mut task));
        assert_data_tree!(
            @executor runner.executor,
            runner.inspector,
            root: {
                MetricsLogger: {
                    test: {
                    }
                }
            }
        );
    }
}
