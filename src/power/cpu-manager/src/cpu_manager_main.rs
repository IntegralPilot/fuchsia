// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::common_utils::result_debug_panic::ResultDebugPanic;
use crate::error::CpuManagerError;
use crate::message::{Message, MessageResult, MessageReturn};
use crate::node::Node;
use crate::ok_or_default_err;
use crate::types::{NormPerfs, OperatingPoint, ThermalLoad, Watts};
use anyhow::{Error, anyhow, bail, format_err};
use async_trait::async_trait;
use async_utils::event::Event as AsyncEvent;
use fidl_fuchsia_power_cpu as fcpu;
use fidl_fuchsia_thermal as fthermal;
use fuchsia_inspect::{self as inspect, ArrayProperty as _, Property as _};
use futures::lock::{Mutex, MutexGuard};
use serde_derive::Deserialize;
use serde_json as json;
use state_recorder::{EnumStateRecorder, RecorderOptions};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::convert::TryInto as _;
use std::fmt::Debug;
use std::rc::Rc;
use strum_macros::{Display, EnumIter, EnumString, FromRepr};
use zx::sys;

/// Node: CpuManagerMain
///
/// Summary: Provides high-level management of all CPU domains in the system, coordinating both
///          driver- and kernel-level activity. Currently only administers CPU throttling, but in
///          the longer term is meant to become a standalone component that provides a FIDL
///          interface for managing DVFS.
///
/// Handles Messages:
///     - GetDomainInfos
///     - SetBoost
///     - SetMaxFrequency
///     - UpdateThermalLoad
///
/// Sends Messages:
///     - GetCpuLoads
///     - GetCpuOperatingPoints
///     - GetOperatingPoint
///     - SetCpuPerformanceInfo
///     - SetOperatingPoint
///     - SetMinimumOperatingPointLimit
///
/// FIDL dependencies: No direct dependencies

// NOTE(https://fxbug.dev/42166756): The thermal state configuration for Sherlock is generated using
// a process described in this bug.
// TODO(https://fxbug/dev/85813): Move this comment to the sherlock node config.

#[derive(Clone, Copy, Debug)]
struct Range<T: Clone + Copy + Debug> {
    upper: T,
    lower: T,
}

/// Describes a value that, if not known exactly, can be confined to a range.
#[derive(Clone, Copy, Debug)]
enum RangedValue<T: Clone + Copy + Debug> {
    Known(T),
    InRange(Range<T>),
}

// The `lower()` and `upper()` methods allow a Known value to be treated as a singleton range
// without adding extra complexity at the call site.
impl<T: Clone + Copy + Debug> RangedValue<T> {
    fn lower(&self) -> T {
        match self {
            &Self::Known(value) => value,
            &Self::InRange(range) => range.lower,
        }
    }

    fn upper(&self) -> T {
        match self {
            &Self::Known(value) => value,
            &Self::InRange(range) => range.upper,
        }
    }
}

/// Runtime representation of a CPU cluster.
struct CpuCluster {
    /// Name of the cluster, for logging purposes only.
    name: String,

    /// This cluster's index in CpuManagerMain's ordering of all clusters.
    cluster_index: usize,

    /// Handler that manages the corresponding CPU driver. Must respond to:
    ///  - GetOperatingPoint
    ///  - SetOperatingPoint
    ///  - GetCpuOperatingPoints
    handler: Rc<dyn Node>,

    /// Logical CPU numbers of the CPUs in this cluster.
    logical_cpu_numbers: Vec<u32>,

    /// Normalized performance of this cluster per GHz of CPU speed.
    performance_per_ghz: NormPerfs,

    /// All opps supported by this cluster. The handler guarantees that they are sorted
    /// primairly by frequency and secondarily by voltage.
    opps: Vec<OperatingPoint>,

    /// Index of this cluster's current opp. If an update to the opp fails, we will assume
    /// that it is between the previous and desired states (inclusive), so that pessimistic guesses
    /// of the opp may be used accordingly.
    // TODO(https://fxbug.dev/42165500): Look into richer specification of failure modes in the CPU
    // device protocols.
    current_opp: Cell<RangedValue<usize>>,

    /// The max frequency limit OPP that this cluster can run at.
    max_frequency_limit_opp_index: Option<u64>,
}

impl CpuCluster {
    /// Given fractional loads for all system CPUs, gives this cluster's corresponding load and its
    /// estimated normalized performance. If the current opp is unknown, the highest-possible
    /// frequency will be used to ensure that performance (and thus contribution to thermals) is
    /// not underestimated.
    fn process_fractional_loads(&self, all_cpu_loads: &Vec<f32>) -> (f32, NormPerfs) {
        let cluster_load: f32 =
            self.logical_cpu_numbers.iter().map(|i| all_cpu_loads[*i as usize]).sum();

        // opps are sorted with frequency as primary key, so the lowest-possible index has the
        // highest-possible frequency.
        let opp_index = self.current_opp.get().lower();
        let frequency = self.opps[opp_index].frequency;

        let performance =
            self.performance_per_ghz.mul_scalar(frequency.0 / 1e9 * cluster_load as f64);
        (cluster_load, performance)
    }

    /// Gets the performance capacity of the indicated opp.
    fn get_performance_capacity(&self, opp_index: usize) -> NormPerfs {
        let opp = &self.opps[opp_index];
        let num_cores = self.logical_cpu_numbers.len() as f64;
        self.performance_per_ghz.mul_scalar(num_cores * opp.frequency.0 / 1e9)
    }

    // Updates the kernel's CPU performance info to match the provided opp.
    async fn update_kernel_performance_info(
        &self,
        syscall_handler: &Rc<dyn Node>,
        target_opp: &OperatingPoint,
    ) -> Result<(), CpuManagerError> {
        let performance_scale: sys::zx_cpu_performance_scale_t =
            self.performance_per_ghz.mul_scalar(target_opp.frequency.0 / 1e9).try_into()?;

        let performance_info = self
            .logical_cpu_numbers
            .iter()
            .map(|n| sys::zx_cpu_performance_info_t { logical_cpu_number: *n, performance_scale })
            .collect::<Vec<_>>();

        let msg = Message::SetCpuPerformanceInfo(performance_info);
        match syscall_handler.handle_message(&msg).await {
            Ok(MessageReturn::SetCpuPerformanceInfo) => Ok(()),
            Ok(other) => panic!("Unexpected SetCpuPerformanceInfo result: {:?}", other),
            Err(e) => Err(e),
        }
    }

    // Carries out a opp change for this cluster.
    async fn update_opp(
        &self,
        syscall_handler: &Rc<dyn Node>,
        mut index: usize,
    ) -> Result<(), CpuManagerError> {
        // Constrain the index if it indicates an OPP higher than the maximum. OPPs are sorted from
        // highest frequency to lowest, so a lower index indicates a higher frequency.
        if let Some(ref opp_limit_index) = self.max_frequency_limit_opp_index {
            index = index.max(*opp_limit_index as usize);
        }

        fuchsia_trace::counter!(
            c"cpu_manager",
            c"CpuManagerMain opp",
            self.cluster_index as u64,
            &self.name => index as u32
        );

        // If the current opp is known and equal to the new one, no update is needed.
        if let RangedValue::Known(current) = self.current_opp.get() {
            if current == index {
                return Ok(());
            }
        }

        // If the opp is unknown, the lowest-possible frequency (highest-possible opp index)
        // is what was used to inform the last `update_kernel_performance_info` call.
        let current_frequency = self.opps[self.current_opp.get().upper()].frequency;

        let target_opp = &self.opps[index];

        // If lowering frequency, we update the kernel before changing opps. Otherwise, the
        // kernel will be updated after the change.
        let kernel_updated = if target_opp.frequency < current_frequency {
            self.update_kernel_performance_info(&syscall_handler, target_opp).await?;
            true
        } else {
            false
        };

        // If the current opp is unknown or not equal to the new one, attempt an update.
        match self.handler.handle_message(&Message::SetOperatingPoint(index as u32)).await {
            Ok(MessageReturn::SetOperatingPoint) => {
                self.current_opp.set(RangedValue::Known(index));

                if !kernel_updated {
                    self.update_kernel_performance_info(&syscall_handler, target_opp).await?;
                }
                Ok(())
            }
            Ok(r) => {
                // Programming error
                panic!("Wrong response type for SetOperatingPoint: {:?}", r);
            }
            Err(e) => {
                log::error!("SetOperatingPoint failed: {:?}", e);

                // If the update failed, query the value, so at least the current state is known. If
                // that fails, too, record a range of possible values based on the previous and
                // desired values. Regardless, propagate the error from SetOperatingPoint.
                match self.handler.handle_message(&Message::GetOperatingPoint).await {
                    Ok(MessageReturn::GetOperatingPoint(i)) => {
                        self.current_opp.set(RangedValue::Known(i as usize));
                    }
                    result => {
                        log::error!("Unexpected result from GetOperatingPoint: {:?}", result);
                        let range = Range {
                            lower: std::cmp::min(self.current_opp.get().lower(), index),
                            upper: std::cmp::max(self.current_opp.get().upper(), index),
                        };
                        self.current_opp.set(RangedValue::InRange(range));
                    }
                }

                // If we already updated the kernel, make a new update using the lowest-possible
                // CPU frequency (highest-possbile opp index) to provide a pessimistic estimate
                // of CPU performance.
                if kernel_updated {
                    let opp = &self.opps[self.current_opp.get().upper()];
                    self.update_kernel_performance_info(&syscall_handler, opp).await?;
                }

                Err(e)
            }
        }
    }
}

/// Cross-cluster CPU thermal state
///
/// The power of a thermal state for a given performance in NormPerfs is modeled as
//      power = static_power + dynamic_power_per_normperf * performance.
#[derive(Clone, Debug)]
struct ThermalState {
    /// Index of the opp to be used for each CPU cluster.
    cluster_opps: Vec<usize>,

    /// Minimum performance at which this thermal state will be used. At low performance values,
    /// it is common for different thermal states to have very similar power requirements. The
    /// minimum performance is used to force a particular choice between such states.
    min_performance: NormPerfs,

    /// Static (fixed) power draw of this thermal state.
    static_power: Watts,

    /// Power draw per unit of normalized performance, assuming that load is perfectly balanced
    /// across CPUs.
    ///
    /// If there is only one cluster, and it is modeled using the standard switching power model
    ///     switching_power = capacitance * voltage**2 * operation_rate,
    /// then dynamic_power_per_normperf would be
    ///     operation_rate * performance_per_ghz * capacitance * voltage**2.
    /// The multi-cluster case is somewhat more complicated, and we furthermore don't require use
    /// of the switching power model. But the voltage term captures a typical way that this value
    /// depends on the underlying opps.
    dynamic_power_per_normperf: Watts,

    /// Maximum performance that this thermal state can provide, i.e. the performance that will be
    /// achieved when all CPUs are saturated. This value is derived directly from the opps
    /// specified by this thermal state.
    ///
    /// The term "capacity" is used in agreement with the kernel scheduler.
    performance_capacity: NormPerfs,
}

struct PerfAndPower {
    performance: NormPerfs,
    power: Watts,
}

impl ThermalState {
    /// Estimates the performance and power of this thermal state for the given model of desired
    /// performance.
    fn estimate_perf_and_power(&self, performance_model: PerformanceModel) -> PerfAndPower {
        let performance = match performance_model {
            Saturated => self.performance_capacity,
            FixedValue(p) => NormPerfs::min(self.performance_capacity, p),
        };
        let dynamic_power = self.dynamic_power_per_normperf.mul_scalar(performance.0);

        PerfAndPower { performance, power: self.static_power + dynamic_power }
    }

    /// Like `estimate_perf_and_power`, but returns only the power estimate.
    fn estimate_power(&self, performance_model: PerformanceModel) -> Watts {
        self.estimate_perf_and_power(performance_model).power
    }
}

/// Validates that a list of thermal states are valid, meeting the criteria:
///  - State 0 has min_performance of 0.0;
///  - For any input `desired_performance`, the admissible states (states for which
///    `state.min_performance <= desired_performance`) are in order of strictly decreasing modeled
///    power.
///
/// To address the power criterion, two further definitions are necessary:
///  - Power is modeled as
///
///    ```
///      power = static_power
///              + dynamic_power_per_normperf * min(desired_performance, performance_capacity).
///    ```
///
///  - Two states are *adjacent* at `desired_performance` if and only if they are both admissible at
///    `desired_performance`, and no state between them in the full list of states is admissible.
///
/// Power is strictly decreasing if the following conditions are met:
///  - dynamic_power_per_normperf is non-increasing;
///  - performance_capacity is non-increasing;
///  - When any two states are adjacent, the lower-index state draws more power at the first
///    performance value at which they are adjacent.
///
/// The first two conditions guarantee that the power delta between a lower-index state and a
/// higher-index state is non-decreasing with respect to performance. The third condition implies
/// that the power delta between any two adjacent states is initially positive; since the delta is
/// non-decreasing by the first two conditions, it will continue to be positive for all subsequent
/// performance values.
fn validate_thermal_states(states: &Vec<ThermalState>) -> Result<(), Error> {
    anyhow::ensure!(
        states[0].min_performance == NormPerfs(0.0),
        "State 0 ({:?}) must have min_performance == 0.0.",
        states[0]
    );

    for i in 0..states.len() - 1 {
        let state = &states[i];
        let next_state = &states[i + 1];

        anyhow::ensure!(
            next_state.dynamic_power_per_normperf <= state.dynamic_power_per_normperf,
            "Thermal states' dynamic_power_per_normperf must be non-increasing; violated by \
            {:?} and {:?}",
            state,
            next_state
        );
        anyhow::ensure!(
            next_state.performance_capacity <= state.performance_capacity,
            "Thermal states' performance_capacity must be non-increasing; violated by {:?} and \
            {:?}",
            state,
            next_state
        );

        // Compare `state` to each state further down the list that will be adjacent to it at some
        // performance value. Verify that the adjacent state has lower power at the first shared
        // performance.
        for adjacent_state in &states[i + 1..] {
            let first_shared_performance =
                FixedValue(NormPerfs::max(state.min_performance, adjacent_state.min_performance));
            let power = state.estimate_power(first_shared_performance);
            let adjacent_power = adjacent_state.estimate_power(first_shared_performance);

            anyhow::ensure!(
                adjacent_power < power,
                "Power is not strictly decreasing at performance {:?}; state {:?} consumes {:?}, \
                while state {:?} consumes {:?}.",
                first_shared_performance,
                state,
                power,
                adjacent_state,
                adjacent_power
            );

            if adjacent_state.min_performance <= state.min_performance {
                // `adjacent_state` is always between `state` and any state later in the list.
                break;
            }
        }
    }

    Ok(())
}

// Configuration structs for CpuManagerMainBuilder.
#[derive(Clone, Debug, Deserialize)]
struct ClusterConfig {
    name: String,
    cluster_index: usize,
    handler: String,
    logical_cpu_numbers: Vec<u32>,
    normperfs_per_ghz: f64,
}

#[derive(Clone, Deserialize)]
struct ThermalStateConfig {
    cluster_opps: Vec<usize>,
    min_performance_normperfs: f64,
    static_power_w: f64,
    dynamic_power_per_normperf_w: f64,
}

#[derive(Deserialize)]
struct Config {
    sustainable_power: f64,
    power_gain: f64,
    clusters: Vec<ClusterConfig>,
    thermal_states: Vec<ThermalStateConfig>,
}

#[derive(Deserialize)]
struct Dependencies {
    cpu_device_handlers: Vec<String>,
    cpu_stats_handler: String,
    syscall_handler: String,
}

#[derive(Deserialize)]
struct JsonData {
    config: Config,
    dependencies: Dependencies,
}

/// Builder for `CpuManagerMain`
pub struct CpuManagerMainBuilder<'a> {
    sustainable_power: Watts,
    power_gain: Watts,
    cluster_configs: Vec<ClusterConfig>,

    /// Parallel to `cluster_configs`; contains one `CpuDeviceHandler` node (or equivalent) for each
    /// CPU cluster.
    cluster_handlers: Vec<Rc<dyn Node>>,

    thermal_state_configs: Vec<ThermalStateConfig>,
    syscall_handler: Rc<dyn Node>,
    cpu_stats_handler: Rc<dyn Node>,
    inspect_root: Option<&'a inspect::Node>,
}

impl<'a> CpuManagerMainBuilder<'a> {
    pub fn new_from_json(json_data: json::Value, nodes: &HashMap<String, Rc<dyn Node>>) -> Self {
        let data: JsonData = json::from_value(json_data).unwrap();
        assert_eq!(
            data.config.clusters.iter().map(|c| &c.handler).collect::<Vec<_>>(),
            data.dependencies.cpu_device_handlers.iter().collect::<Vec<_>>()
        );

        let cluster_handlers =
            data.config.clusters.iter().map(|c| nodes[&c.handler].clone()).collect();
        let cpu_stats_handler = nodes[&data.dependencies.cpu_stats_handler].clone();
        let syscall_handler = nodes[&data.dependencies.syscall_handler].clone();

        Self::new(
            Watts(data.config.sustainable_power),
            Watts(data.config.power_gain),
            data.config.clusters,
            cluster_handlers,
            data.config.thermal_states,
            syscall_handler,
            cpu_stats_handler,
        )
    }

    fn new(
        sustainable_power: Watts,
        power_gain: Watts,
        cluster_configs: Vec<ClusterConfig>,
        cluster_handlers: Vec<Rc<dyn Node>>,
        thermal_state_configs: Vec<ThermalStateConfig>,
        syscall_handler: Rc<dyn Node>,
        cpu_stats_handler: Rc<dyn Node>,
    ) -> Self {
        Self {
            sustainable_power,
            power_gain,
            cluster_configs,
            cluster_handlers,
            thermal_state_configs,
            cpu_stats_handler,
            syscall_handler,
            inspect_root: None,
        }
    }

    #[cfg(test)]
    pub fn with_inspect_root(mut self, root: &'a inspect::Node) -> Self {
        self.inspect_root = Some(root);
        self
    }

    pub fn build(self) -> Result<Rc<CpuManagerMain>, Error> {
        // Optionally use the default inspect root node
        let inspect_root =
            self.inspect_root.unwrap_or_else(|| inspect::component::inspector().root());

        let cluster_names = self.cluster_configs.iter().map(|c| c.name.as_str()).collect();
        let inspect = InspectData::new(inspect_root, "CpuManagerMain", cluster_names);

        let mut throttling_state_recorder = EnumStateRecorder::new(
            "cpu_throttling_state".into(),
            c"thermal",
            RecorderOptions {
                capacity: 50,
                lazy_record: true,
                manager: None,
                ..Default::default()
            },
        )?;
        throttling_state_recorder.record(ThrottlingState::Inactive);

        let mutable_inner = MutableInner {
            num_cpus: 0,
            current_thermal_state: None,
            cluster_configs: Some(self.cluster_configs),
            cluster_handlers: Some(self.cluster_handlers),
            clusters: Vec::new(),
            thermal_state_configs: Some(self.thermal_state_configs),
            thermal_states: Vec::new(),
            throttling_state_recorder,
            active_boosts: HashSet::new(),
        };

        Ok(Rc::new(CpuManagerMain {
            sustainable_power: self.sustainable_power,
            power_gain: self.power_gain,
            init_done: AsyncEvent::new(),
            cpu_stats_handler: self.cpu_stats_handler,
            syscall_handler: self.syscall_handler,
            inspect,
            mutable_inner: Mutex::new(mutable_inner),
        }))
    }

    #[cfg(test)]
    pub async fn build_and_init(self) -> Rc<CpuManagerMain> {
        let node = self.build().unwrap();
        node.init().await.unwrap();
        node
    }
}

const BOOST_OPP: usize = 0;

pub struct CpuManagerMain {
    /// The available power when thermal load is 0.
    sustainable_power: Watts,

    /// A scale factor that maps thermal load to available power.
    power_gain: Watts,

    /// Signalled after `init()` has completed. Used to ensure node doesn't process messages until
    /// its `init()` has completed.
    init_done: AsyncEvent,

    /// Must service GetNumCpus and SetCpuPerformanceInfo messages.
    syscall_handler: Rc<dyn Node>,

    /// The node that will provide CPU load information. It is expected that this node responds to
    /// the GetCpuLoads message.
    cpu_stats_handler: Rc<dyn Node>,

    inspect: InspectData,

    /// Mutable inner state. This must be guarded with a Mutex rather than a RefCell because CPU
    /// boost requests can overlap with thermal updates.
    mutable_inner: Mutex<MutableInner>,
}

/// A performance model for a future time interval.
#[derive(Copy, Clone, Debug)]
enum PerformanceModel {
    /// Models performance to be a fixed value.
    FixedValue(NormPerfs),

    /// Models the CPUs to be saturated regardless of frequency.
    Saturated,
}
use PerformanceModel::{FixedValue, Saturated};

impl PerformanceModel {
    /// Indicates whether a fractional CPU load is considered saturated.
    fn cpu_load_is_saturated(load: f32) -> bool {
        debug_assert!(load <= 1.0);
        const CPU_SATURATION_FRACTION: f32 = 0.99;
        load > CPU_SATURATION_FRACTION
    }
}

impl CpuManagerMain {
    // Returns a Vec of all CPU loads as fractional utilizations.
    async fn get_cpu_loads(&self) -> Result<Vec<f32>, Error> {
        fuchsia_trace::duration!("cpu_manager", "CpuManagerMain::get_cpu_loads");

        // Get load for all CPUs in the system
        match self.send_message(&self.cpu_stats_handler, &Message::GetCpuLoads).await {
            Ok(MessageReturn::GetCpuLoads(loads)) => Ok(loads),
            Ok(r) => Err(format_err!("GetCpuLoads had unexpected return value: {:?}", r)),
            Err(e) => Err(format_err!("GetCpuLoads failed: {:?}", e)),
        }
    }

    // Determines the thermal state that should be used for the given available power and
    // performance model, as well as its estimated performance and power.
    fn select_thermal_state(
        thermal_states: &Vec<ThermalState>,
        available_power: Watts,
        performance_model: PerformanceModel,
    ) -> (usize, PerfAndPower) {
        // State 0 is guaranteed to be admissible. If it meets the power criterion, we return it.
        // Otherwise, we use it to initialize the fallback -- the lowest-index admissible state,
        // which will be used if no states meet the power criterion.
        debug_assert_eq!(thermal_states[0].min_performance, NormPerfs(0.0));
        let mut fallback = (0, thermal_states[0].estimate_perf_and_power(performance_model));
        if fallback.1.power < available_power {
            return fallback;
        }

        for (i, thermal_state) in thermal_states.iter().enumerate().skip(1) {
            if let FixedValue(performance) = performance_model {
                if thermal_state.min_performance > performance {
                    continue;
                }
            }

            let estimate = thermal_state.estimate_perf_and_power(performance_model);
            if estimate.power < available_power {
                return (i, estimate);
            }

            fallback = (i, estimate);
        }

        fallback
    }

    // Updates the current thermal state.
    async fn update_thermal_state(&self, index: usize) -> Result<(), CpuManagerError> {
        fuchsia_trace::duration!(
            c"cpu_manager",
            c"CpuManagerMain::update_thermal_state",
            "index" => index as u32
        );

        let mut inner = self.mutable_inner.lock().await;

        if let Some(current_index) = inner.current_thermal_state {
            // Return early if no update is required. We're assuming that opps have not changed.
            if current_index == index {
                return Ok(());
            }
            // A positive index indicates a throttled state. So the first condition describes
            // the transition from unthrottled to throttled, while the second describes the
            // transition from throttled to unthrottled. (There is some redundant logic in the
            // conditions to improve clarity.)
            if current_index == 0 && index > 0 {
                inner.throttling_state_recorder.record(ThrottlingState::Active);
                log::info!("Starting CPU throttling because thermal state is {index}")
            }
            if current_index > 0 && index == 0 {
                inner.throttling_state_recorder.record(ThrottlingState::Inactive);
                log::info!("Ending CPU throttling");
            }
        }

        // Update the thermal state index.
        inner.current_thermal_state = Some(index);
        match self.update_cluster_opps(&inner, index).await {
            Ok(_) => {
                self.inspect.thermal_state_index.set(&index.to_string());
                Ok(())
            }
            Err(e) => {
                inner.current_thermal_state = None;

                self.inspect.thermal_state_index.set(&format!("Unknown; {:?}", e));
                Err(e)
            }
        }
    }

    /// Handles an `UpdateThermalLoad` message.
    ///
    /// The new thermal load is checked for validity then used to calculate the available power.
    async fn handle_update_thermal_load(
        &self,
        thermal_load: ThermalLoad,
    ) -> Result<MessageReturn, CpuManagerError> {
        fuchsia_trace::duration!(
            c"cpu_manager",
            c"CpuManagerMain::handle_update_thermal_load",
            "thermal_load" => thermal_load.0
        );

        if thermal_load > ThermalLoad(fthermal::MAX_THERMAL_LOAD) {
            return Err(CpuManagerError::InvalidArgument(format!(
                "Thermal load {:?} exceeds max {}",
                thermal_load,
                fthermal::MAX_THERMAL_LOAD
            )));
        }

        let available_power = self.calculate_available_power(thermal_load);
        fuchsia_trace::counter!(
            c"cpu_manager",
            c"CpuManagerMain available_power",
            0,
            "available_power" => available_power.0
        );
        self.set_max_power_consumption(available_power).await;

        Ok(MessageReturn::UpdateThermalLoad)
    }

    /// Calculate available power based on thermal load.
    fn calculate_available_power(&self, thermal_load: ThermalLoad) -> Watts {
        fuchsia_trace::duration!(
            c"cpu_manager",
            c"CpuManagerMain::calculate_available_power",
            "thermal_load" => thermal_load.0
        );

        let power_available =
            f64::max(0.0, self.sustainable_power.0 - thermal_load.0 as f64 * self.power_gain.0);

        Watts(power_available)
    }

    /// If an error is encountered in the execution of this method, CpuManagerMain will keep itself in
    /// a usable state by using pessimistic estimates of any value that it cannot determine and then
    /// log the error.
    async fn set_max_power_consumption(&self, available_power: Watts) {
        fuchsia_trace::duration!(
            c"cpu_manager",
            c"CpuManagerMain::set_max_power_consumption",
            "available_power" => available_power.0
        );

        self.init_done.wait().await;

        self.inspect.available_power.set(available_power.0);

        // Gather CPU loads over the last time interval. In the unlikely event of an error, use the
        // worst-case CPU load of 1.0 for all CPUs and throttle accordingly.
        let cpu_loads = match self.get_cpu_loads().await {
            Ok(loads) => loads,
            Err(e) => {
                log::error!(
                    "Error querying CPU loads: {}\nWill throttle assuming maximal load.",
                    e
                );
                self.inspect.last_error.set(&e.to_string());
                vec![1.0; self.mutable_inner.lock().await.num_cpus as usize]
            }
        };

        // Determine the normalized performance over the last interval.
        let mut last_performance = NormPerfs(0.0);
        for cluster in self.mutable_inner.lock().await.clusters.iter() {
            let (load, performance) = cluster.process_fractional_loads(&cpu_loads);
            last_performance += performance;

            fuchsia_trace::counter!(
                c"cpu_manager",
                c"CpuManagerMain cluster_load",
                cluster.cluster_index as u64,
                &cluster.name => load as f64
            );
            self.inspect.last_cluster_loads[cluster.cluster_index].set(load as f64);
        }
        self.inspect.last_performance.set(last_performance.0);
        fuchsia_trace::counter!(
            c"cpu_manager",
            c"CpuManagerMain last_performance",
            0,
            "value (NormPerfs)" => last_performance.0
        );

        let cpus_saturated = cpu_loads.iter().all(|l| PerformanceModel::cpu_load_is_saturated(*l));
        let performance_model =
            if cpus_saturated { Saturated } else { FixedValue(last_performance) };

        // Determine the next thermal state, updating if needed. We use the performance over the
        // last interval as an estimate of performance over the next interval; in principle a more
        // sophisticated estimate could be used.
        let (new_thermal_state_index, estimate) = Self::select_thermal_state(
            &self.mutable_inner.lock().await.thermal_states,
            available_power,
            performance_model,
        );
        fuchsia_trace::counter!(
            c"cpu_manager",
            c"CpuManagerMain new_thermal_state_index",
            0,
            "value" => new_thermal_state_index as u32
        );

        if let Err(e) = self.update_thermal_state(new_thermal_state_index).await {
            self.inspect.last_error.set(&e.to_string());
            log::error!("Error updating thermal states: {}", e);
            return;
        }

        fuchsia_trace::counter!(
            c"cpu_manager",
            c"CpuManagerMain projected_performance",
            0,
            "value (NormPerfs)" => estimate.performance.0
        );
        self.inspect.projected_performance.set(estimate.performance.0);

        fuchsia_trace::counter!(
            c"cpu_manager",
            c"CpuManagerMain projected_power",
            0,
            "value (W)" => estimate.power.0
        );
        self.inspect.projected_power.set(estimate.power.0);
    }

    async fn handle_set_boost(
        &self,
        enable: bool,
        id: u64,
    ) -> Result<MessageReturn, CpuManagerError> {
        fuchsia_trace::duration!(
            c"cpu_manager",
            c"CpuManagerMain::handle_set_boost",
            "enable" => enable,
            "id" => id
        );

        let mut inner = self.mutable_inner.lock().await;
        let was_enabled = !inner.active_boosts.is_empty();

        if enable {
            inner.active_boosts.insert(id);
        } else {
            inner.active_boosts.remove(&id);
        }

        let should_be_enabled = !inner.active_boosts.is_empty();

        if should_be_enabled == was_enabled {
            return Ok(MessageReturn::SetBoost);
        }

        self.inspect.boost_enabled.set(should_be_enabled);

        let messages: Vec<_> = inner
            .clusters
            .iter()
            .map(|cluster| {
                let min_opp = if should_be_enabled { BOOST_OPP } else { cluster.opps.len() - 1 };
                Message::SetMinimumOperatingPointLimit(min_opp as u32)
            })
            .collect();

        let futures: Vec<_> = inner
            .clusters
            .iter()
            .zip(messages.iter())
            .map(|(cluster, msg)| cluster.handler.handle_message(msg))
            .collect();

        let results = futures::future::join_all(futures).await;
        let errors: Vec<_> = results.into_iter().filter_map(|r| r.err()).collect();

        if !errors.is_empty() {
            return Err(CpuManagerError::GenericError(anyhow!(
                "Failed to set minimum operating points for boost={:?}: {:?}",
                should_be_enabled,
                errors
            )));
        }

        Ok(MessageReturn::SetBoost)
    }

    async fn handle_get_domain_infos(&self) -> Result<MessageReturn, CpuManagerError> {
        self.init_done.wait().await;

        let inner = self.mutable_inner.lock().await;

        let cpu_cluster_infos = inner
            .clusters
            .iter()
            .map(|c| fcpu::DomainInfo {
                id: Some(c.cluster_index as u64),
                core_ids: Some(c.logical_cpu_numbers.iter().map(|n| *n as u64).collect()),
                available_frequencies_hz: Some(
                    c.opps.iter().map(|o| o.frequency.0 as u64).collect(),
                ),
                name: Some(c.name.clone()),
                ..Default::default()
            })
            .collect();

        Ok(MessageReturn::GetDomainInfos(cpu_cluster_infos))
    }

    async fn handle_get_max_frequency(
        &self,
        cluster_index: u64,
    ) -> Result<MessageReturn, CpuManagerError> {
        self.init_done.wait().await;
        let mut inner = self.mutable_inner.lock().await;
        let Some(cluster) =
            inner.clusters.iter_mut().find(|c| c.cluster_index == cluster_index as usize)
        else {
            return Err(CpuManagerError::InvalidArgument(format!(
                "cluster with index {} not found",
                cluster_index
            )));
        };

        Ok(MessageReturn::GetMaxFrequency(
            *cluster.max_frequency_limit_opp_index.as_ref().unwrap_or(&0),
        ))
    }

    async fn handle_set_max_frequency(
        &self,
        cluster_index: u64,
        frequency_index: Option<u64>,
    ) -> Result<MessageReturn, CpuManagerError> {
        let mut inner = self.mutable_inner.lock().await;
        let thermal_limited_opps = inner.thermal_states
            [inner.current_thermal_state.unwrap_or_default()]
        .cluster_opps
        .clone();
        let max_thermal_opp = thermal_limited_opps[cluster_index as usize];

        let Some(cluster) =
            inner.clusters.iter_mut().find(|c| c.cluster_index == cluster_index as usize)
        else {
            return Err(CpuManagerError::InvalidArgument(format!(
                "cluster with index {} not found",
                cluster_index
            )));
        };

        let Some(frequency_index) = frequency_index else {
            log::info!("Removing max frequency constraint for cluster {}", cluster_index);
            cluster.max_frequency_limit_opp_index = None;
            cluster.update_opp(&self.syscall_handler, max_thermal_opp).await?;
            return Ok(MessageReturn::SetMaxFrequency);
        };

        log::info!(
            "Setting max frequency constraint to OPP index {} for cluster {}",
            frequency_index,
            cluster_index
        );
        cluster.max_frequency_limit_opp_index = Some(frequency_index);
        cluster.update_opp(&self.syscall_handler, max_thermal_opp).await?;
        Ok(MessageReturn::SetMaxFrequency)
    }

    async fn update_cluster_opps(
        &self,
        inner: &MutexGuard<'_, MutableInner>,
        thermal_state: usize,
    ) -> Result<(), CpuManagerError> {
        fuchsia_trace::duration!(
            c"cpu_manager",
            c"CpuManagerMain::update_cluster_opps",
            "thermal_state" => thermal_state as u32,
        );

        let opp_indices = inner.thermal_states[thermal_state].cluster_opps.clone();

        let cluster_update_futures: Vec<_> = inner
            .clusters
            .iter()
            .map(|cluster| {
                cluster.update_opp(&self.syscall_handler, opp_indices[cluster.cluster_index])
            })
            .collect();

        // Aggregate any errors that may have occurred when setting opps.
        let errors: Vec<_> = futures::future::join_all(cluster_update_futures)
            .await
            .into_iter()
            .filter_map(|r| r.err())
            .collect();

        if !errors.is_empty() {
            return Err(CpuManagerError::GenericError(anyhow!(
                "Failed to update opp(s) ({:?}): {:?}",
                opp_indices,
                errors
            )));
        }
        Ok(())
    }
}

#[derive(Copy, Clone, Debug, Display, EnumIter, EnumString, Eq, PartialEq, Hash, FromRepr)]
#[repr(u8)]
enum ThrottlingState {
    Inactive = 0,
    Active = 1,
}

impl From<ThrottlingState> for u64 {
    fn from(value: ThrottlingState) -> Self {
        value as Self
    }
}

struct MutableInner {
    /// Number of CPUs in the system; confirmed to be greater than the max logical CPU number of any
    /// cluster. Populated by querying the `syscall_handler` node.
    num_cpus: u32,

    /// Cluster configs as supplied in the node config. Wrapped in an Option because they are
    /// consumed to generate `clusters` in `init()`.
    cluster_configs: Option<Vec<ClusterConfig>>,

    /// Parallel to `cluster_configs`; contains one `CpuDeviceHandler` node (or equivalent) for each
    /// CPU cluster.
    cluster_handlers: Option<Vec<Rc<dyn Node>>>,

    /// All CPU clusters governed by the `CpuManagerMain`.
    clusters: Vec<CpuCluster>,

    /// Thermal state configs as supplied in the node config. Wrapped in an Option because they are
    /// consumed to generate `thermal_states` in `init()`.
    thermal_state_configs: Option<Vec<ThermalStateConfig>>,

    /// All supported thermal states for the CPU subsystem.
    thermal_states: Vec<ThermalState>,

    /// The current thermal state of the CPU subsystem. The CPU will be put into its highest-power
    /// state during `init()`.
    current_thermal_state: Option<usize>,

    /// Records throttling state to Inspect and trace
    throttling_state_recorder: state_recorder::EnumStateRecorder<ThrottlingState>,

    /// Set of active boost sources.
    active_boosts: HashSet<u64>,
}

#[async_trait(?Send)]
impl Node for CpuManagerMain {
    fn name(&self) -> String {
        "CpuManagerMain".to_string()
    }

    /// Initializes internal state.
    async fn init(&self) -> Result<(), Error> {
        fuchsia_trace::duration!("cpu_manager", "CpuManagerMain::init");

        let cluster_configs =
            ok_or_default_err!(self.mutable_inner.lock().await.cluster_configs.take())
                .or_debug_panic()?;

        let cluster_handlers =
            ok_or_default_err!(self.mutable_inner.lock().await.cluster_handlers.take())
                .or_debug_panic()?;

        let thermal_state_configs =
            ok_or_default_err!(self.mutable_inner.lock().await.thermal_state_configs.take())
                .or_debug_panic()?;

        // Retrieve the total number of CPUs, and ensure that clusters' logical CPU numbers exactly
        // span 0..num_cpus.
        let num_cpus = match self.syscall_handler.handle_message(&Message::GetNumCpus).await {
            Ok(MessageReturn::GetNumCpus(n)) => n,
            other => bail!("Unexpected GetNumCpus response: {:?}", other),
        };
        let mut covered_cpu_numbers = cluster_configs
            .iter()
            .map(|c| c.logical_cpu_numbers.iter())
            .flatten()
            .map(|v| *v)
            .collect::<Vec<_>>();
        covered_cpu_numbers.sort();
        anyhow::ensure!(
            covered_cpu_numbers == (0..num_cpus).collect::<Vec<_>>(),
            "Clusters' logical CPU numbers must exactly span 0..{}",
            num_cpus
        );

        let mut clusters = Vec::new();
        for (cluster_config, handler) in
            cluster_configs.into_iter().zip(cluster_handlers.into_iter())
        {
            let opps = match handler.handle_message(&Message::GetCpuOperatingPoints).await {
                Ok(MessageReturn::GetCpuOperatingPoints(opps)) => opps,
                Ok(r) => {
                    bail!("GetCpuOperatingPoints returned unexpected value: {:?}", r)
                }
                Err(e) => bail!("Error fetching operating points: {}", e),
            };

            // The current opp will be set when CpuManagerMain's thermal state is initialized below,
            // so initialize it to a range of all possible values for now.
            let opp_range = Range { lower: 0, upper: opps.len() - 1 };
            let current_opp = Cell::new(RangedValue::InRange(opp_range));

            clusters.push(CpuCluster {
                name: cluster_config.name,
                cluster_index: cluster_config.cluster_index,
                handler,
                logical_cpu_numbers: cluster_config.logical_cpu_numbers,
                performance_per_ghz: NormPerfs(cluster_config.normperfs_per_ghz),
                opps,
                current_opp,
                max_frequency_limit_opp_index: None,
            });
        }

        let get_performance_capacity = |thermal_state_config: &ThermalStateConfig| {
            clusters
                .iter()
                .map(|cluster| {
                    let opp_index = thermal_state_config.cluster_opps[cluster.cluster_index];
                    cluster.get_performance_capacity(opp_index)
                })
                .sum()
        };

        let thermal_states = thermal_state_configs
            .into_iter()
            .map(|t| {
                let performance_capacity = get_performance_capacity(&t);
                ThermalState {
                    cluster_opps: t.cluster_opps,
                    min_performance: NormPerfs(t.min_performance_normperfs),
                    static_power: Watts(t.static_power_w),
                    dynamic_power_per_normperf: Watts(t.dynamic_power_per_normperf_w),
                    performance_capacity,
                }
            })
            .collect();

        validate_thermal_states(&thermal_states)?;
        self.inspect.set_thermal_states(&thermal_states);

        {
            let mut inner = self.mutable_inner.lock().await;
            inner.num_cpus = num_cpus;
            inner.clusters = clusters;
            inner.thermal_states = thermal_states;
        }

        // Update cluster opps to match the normal power operating condition.
        self.update_thermal_state(0).await?;

        self.init_done.signal();

        Ok(())
    }

    async fn handle_message(&self, msg: &Message) -> MessageResult {
        match msg {
            Message::GetDomainInfos => self.handle_get_domain_infos().await,
            Message::SetBoost(enable, id) => self.handle_set_boost(*enable, *id).await,
            Message::GetMaxFrequency(cluster_index) => {
                self.handle_get_max_frequency(*cluster_index).await
            }
            Message::SetMaxFrequency(cluster_index, frequency_index) => {
                self.handle_set_max_frequency(*cluster_index, *frequency_index).await
            }
            Message::UpdateThermalLoad(thermal_load) => {
                self.handle_update_thermal_load(*thermal_load).await
            }
            _ => Err(CpuManagerError::Unsupported),
        }
    }
}

// TODO(https://fxbug.dev/42165547): Determine whether it would be useful to track histories of any
// of these signals.
struct InspectData {
    root_node: inspect::Node,

    // Properties
    thermal_state_index: inspect::StringProperty,
    last_performance: inspect::DoubleProperty,
    last_cluster_loads: Vec<inspect::DoubleProperty>,
    available_power: inspect::DoubleProperty,
    projected_performance: inspect::DoubleProperty,
    projected_power: inspect::DoubleProperty,
    last_error: inspect::StringProperty,
    boost_enabled: inspect::BoolProperty,
}

impl InspectData {
    fn new(parent: &inspect::Node, node_name: &str, cluster_names: Vec<&str>) -> Self {
        // Create a local root node and properties
        let root_node = parent.create_child(node_name);

        let state_node = root_node.create_child("state");
        let thermal_state_index = state_node.create_string("thermal_state_index", "initializing");
        let last_performance = state_node.create_double("last_performance (NormPerfs)", 0.0);

        let mut last_cluster_loads = Vec::new();
        cluster_names.into_iter().for_each(|name| {
            state_node.record_child(name, |cluster_node| {
                last_cluster_loads.push(cluster_node.create_double("last_load (#cores)", 0.0));
            })
        });

        let available_power = state_node.create_double("available_power (W)", 0.0);
        let projected_performance =
            state_node.create_double("projected_performance (NormPerfs)", 0.0);
        let projected_power = state_node.create_double("projected_power (W)", 0.0);

        let last_error = state_node.create_string("last_error", "<None>");

        let boost_enabled = state_node.create_bool("boost_enabled", false);

        root_node.record(state_node);

        Self {
            root_node,
            thermal_state_index,
            last_performance,
            last_cluster_loads,
            available_power,
            projected_performance,
            projected_power,
            last_error,
            boost_enabled,
        }
    }

    fn set_thermal_states(&self, states: &Vec<ThermalState>) {
        let states_node = self.root_node.create_child("thermal_states");

        // Iterate over `states` in reverse order so that the Inspect nodes appear in the same
        // order as the vector (`create_child` inserts nodes at the head).
        for (i, state) in states.iter().enumerate().rev() {
            let node = states_node.create_child(format!("thermal_state_{:02}", i));

            let opps = node.create_uint_array("cluster_opps", state.cluster_opps.len());
            state.cluster_opps.iter().enumerate().for_each(|(i, p)| opps.set(i, *p as u64));
            node.record(opps);

            node.record_double("min_performance (NormPerfs)", state.min_performance.0);
            node.record_double("static_power (W)", state.static_power.0);
            node.record_double(
                "dynamic_power_per_normperf (W)",
                state.dynamic_power_per_normperf.0,
            );
            node.record_double("performance_capacity (NormPerfs)", state.performance_capacity.0);

            // Pass ownership of the new node to the parent.
            states_node.record(node);
        }

        // Pass ownership of the new `states_node` to the root node
        self.root_node.record(states_node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::mock_node::{MessageMatcher, MockNode, MockNodeMaker, create_dummy_node};
    use crate::types::{Hertz, Volts};
    use crate::{msg_eq, msg_ok_return};
    use assert_matches::assert_matches;
    use diagnostics_assertions::assert_data_tree;
    use test_util::assert_lt;

    // Common test configurations for big and little clusters.
    static BIG_CPU_NUMBERS: [u32; 2] = [0, 1];
    static BIG_OPPS: [OperatingPoint; 3] = [
        OperatingPoint { frequency: Hertz(2.0e9), voltage: Volts(1.0) },
        OperatingPoint { frequency: Hertz(1.9e9), voltage: Volts(0.9) },
        OperatingPoint { frequency: Hertz(1.8e9), voltage: Volts(0.8) },
    ];
    static BIG_PERFORMANCE_PER_GHZ: NormPerfs = NormPerfs(1.0);
    static BIG_CLUSTER_INDEX: usize = 0;

    static LITTLE_CPU_NUMBERS: [u32; 2] = [2, 3];
    static LITTLE_OPPS: [OperatingPoint; 3] = [
        OperatingPoint { frequency: Hertz(1.0e9), voltage: Volts(0.5) },
        OperatingPoint { frequency: Hertz(0.9e9), voltage: Volts(0.4) },
        OperatingPoint { frequency: Hertz(0.8e9), voltage: Volts(0.3) },
    ];
    static LITTLE_PERFORMANCE_PER_GHZ: NormPerfs = NormPerfs(0.5);
    static LITTLE_CLUSTER_INDEX: usize = 1;

    // Select sensible random numbers here.
    // They determine the correlation between thermal load and available power.
    static DEFAULT_SUSTAINABLE_POWER: Watts = Watts(6.22);
    static DEFAULT_POWER_GAIN: Watts = Watts(0.0622046);

    fn make_default_cluster_configs() -> Vec<ClusterConfig> {
        vec![
            ClusterConfig {
                name: "big_cluster".to_string(),
                cluster_index: BIG_CLUSTER_INDEX,
                handler: "<unused>".to_string(),
                logical_cpu_numbers: BIG_CPU_NUMBERS[..].to_vec(),
                normperfs_per_ghz: BIG_PERFORMANCE_PER_GHZ.0,
            },
            ClusterConfig {
                name: "little_cluster".to_string(),
                cluster_index: LITTLE_CLUSTER_INDEX,
                handler: "<unused>".to_string(),
                logical_cpu_numbers: LITTLE_CPU_NUMBERS[..].to_vec(),
                normperfs_per_ghz: LITTLE_PERFORMANCE_PER_GHZ.0,
            },
        ]
    }

    // Convenience struct to manage mocks of the handlers on which CpuManagerMain depends.
    struct Handlers {
        big_cluster: Rc<MockNode>,
        little_cluster: Rc<MockNode>,
        syscall: Rc<MockNode>,
        cpu_stats: Rc<MockNode>,

        // The MockMaker comes last, so it is dropped after the MockNodes.
        _mock_maker: MockNodeMaker,
    }

    impl Handlers {
        fn new() -> Self {
            Handlers::new_with_highest_opp([0, 0])
        }

        fn new_with_highest_opp(highest_opps: [u32; 2]) -> Self {
            let mut mock_maker = MockNodeMaker::new();

            // The big and little cluster handlers are initially queried for all operating points.
            let big_cluster = mock_maker.make(
                "big_cluster_handler",
                vec![(
                    msg_eq!(GetCpuOperatingPoints),
                    msg_ok_return!(GetCpuOperatingPoints(Vec::from(&BIG_OPPS[..]))),
                )],
            );
            let little_cluster = mock_maker.make(
                "little_cluster_handler",
                vec![(
                    msg_eq!(GetCpuOperatingPoints),
                    msg_ok_return!(GetCpuOperatingPoints(Vec::from(&LITTLE_OPPS[..]))),
                )],
            );

            // The syscall handler provides the number of CPUs during initialization.
            let num_cpus = BIG_CPU_NUMBERS.len() + LITTLE_CPU_NUMBERS.len();
            let syscall = mock_maker.make(
                "syscall_handler",
                vec![(msg_eq!(GetNumCpus), msg_ok_return!(GetNumCpus(num_cpus as u32)))],
            );

            let cpu_stats = mock_maker.make("cpu_stats_handler", Vec::new());

            let handlers =
                Self { big_cluster, little_cluster, syscall, cpu_stats, _mock_maker: mock_maker };

            // During initialization, CpuManagerMain configures the highest-power thermal state,
            // with both clusters at their respective opp.
            handlers.expect_big_opp(highest_opps[0]);
            handlers.expect_little_opp(highest_opps[1]);

            handlers
        }

        // Tells the syscall handler to expect a SetCpuPerformanceInfo call for the provided
        // collection of CPUs and performance scale.
        fn expect_performance_scale(&self, logical_cpu_numbers: &[u32], float_scale: f64) {
            let scale = NormPerfs(float_scale).try_into().unwrap();
            let info = logical_cpu_numbers
                .iter()
                .map(|n| sys::zx_cpu_performance_info_t {
                    logical_cpu_number: *n,
                    performance_scale: scale,
                })
                .collect::<Vec<_>>();
            self.syscall.add_msg_response_pair((
                msg_eq!(SetCpuPerformanceInfo(info)),
                msg_ok_return!(SetCpuPerformanceInfo),
            ));
        }

        // Updates the handlers with expectations for a big cluster opp change.
        fn expect_big_opp(&self, opp_index: u32) {
            self.big_cluster.add_msg_response_pair((
                msg_eq!(SetOperatingPoint(opp_index)),
                msg_ok_return!(SetOperatingPoint),
            ));
            let frequency = &BIG_OPPS[opp_index as usize].frequency;
            let float_scale = BIG_PERFORMANCE_PER_GHZ.0 * frequency.0 / 1e9;
            self.expect_performance_scale(&BIG_CPU_NUMBERS, float_scale);
        }

        // Updates the handlers with expectations for a little cluster opp change.
        fn expect_little_opp(&self, opp_index: u32) {
            self.little_cluster.add_msg_response_pair((
                msg_eq!(SetOperatingPoint(opp_index)),
                msg_ok_return!(SetOperatingPoint),
            ));
            let frequency = &LITTLE_OPPS[opp_index as usize].frequency;
            let float_scale = LITTLE_PERFORMANCE_PER_GHZ.0 * frequency.0 / 1e9;
            self.expect_performance_scale(&LITTLE_CPU_NUMBERS, float_scale);
        }

        // Prepares the stats handler for a CPU load query.
        fn enqueue_cpu_loads(&self, loads: Vec<f32>) {
            self.cpu_stats
                .add_msg_response_pair((msg_eq!(GetCpuLoads), msg_ok_return!(GetCpuLoads(loads))));
        }
    }

    // Verify that a node is successfully constructed from JSON configuration.
    #[fuchsia::test]
    async fn test_new_from_json() {
        let handlers = Handlers::new();

        let mut nodes = HashMap::<String, Rc<dyn Node>>::new();
        nodes.insert("big_cluster_node".to_string(), handlers.big_cluster.clone());
        nodes.insert("little_cluster_node".to_string(), handlers.little_cluster.clone());
        nodes.insert("syscall_handler_node".to_string(), handlers.syscall.clone());
        nodes.insert("stats_handler_node".to_string(), handlers.cpu_stats.clone());

        let json_data = json::json!({
            "type": "CpuManagerMain",
            "name": "cpu_manager",
            "config": {
                "sustainable_power": DEFAULT_SUSTAINABLE_POWER.0,
                "power_gain": DEFAULT_POWER_GAIN.0,
                "clusters": [
                      {
                          "name": "big_cluster",
                          "cluster_index": 0,
                          "handler": "big_cluster_node",
                          "logical_cpu_numbers": [0, 1],
                          "normperfs_per_ghz": BIG_PERFORMANCE_PER_GHZ.0
                      },
                      {
                          "name": "little_cluster",
                          "cluster_index": 1,
                          "handler": "little_cluster_node",
                          "logical_cpu_numbers": [2, 3],
                          "normperfs_per_ghz": LITTLE_PERFORMANCE_PER_GHZ.0
                      }
                ],
                "thermal_states": [
                    {
                      "cluster_opps": [0, 0],
                      "min_performance_normperfs": 0.0,
                      "static_power_w": 0.9,
                      "dynamic_power_per_normperf_w": 0.6
                    }
                ]
            },
            "dependencies": {
                "cpu_device_handlers": [
                    "big_cluster_node",
                    "little_cluster_node"
                ],
                "cpu_stats_handler": "stats_handler_node",
                "syscall_handler": "syscall_handler_node"
            }
        });

        let builder = CpuManagerMainBuilder::new_from_json(json_data, &nodes);
        builder.build_and_init().await;
    }

    // Verifies that thermal states are properly validated.
    #[fuchsia::test]
    async fn test_thermal_state_validation() {
        // Since CpuManagerMainBuilder::build() exits early, we need a custom constructor for Handlers
        // that omits expectations for messages that are never sent.
        fn new_handlers_for_failed_validation() -> Handlers {
            let mut mock_maker = MockNodeMaker::new();

            // The big and little cluster handlers are initially queried for all performance
            // states.
            let big_cluster = mock_maker.make(
                "big_cluster_handler",
                vec![(
                    msg_eq!(GetCpuOperatingPoints),
                    msg_ok_return!(GetCpuOperatingPoints(Vec::from(&BIG_OPPS[..]))),
                )],
            );
            let little_cluster = mock_maker.make(
                "little_cluster_handler",
                vec![(
                    msg_eq!(GetCpuOperatingPoints),
                    msg_ok_return!(GetCpuOperatingPoints(Vec::from(&LITTLE_OPPS[..]))),
                )],
            );

            // The syscall handler provides the number of CPUs during initialization.
            let num_cpus = BIG_CPU_NUMBERS.len() + LITTLE_CPU_NUMBERS.len();
            let syscall = mock_maker.make(
                "syscall_handler",
                vec![(msg_eq!(GetNumCpus), msg_ok_return!(GetNumCpus(num_cpus as u32)))],
            );

            Handlers {
                big_cluster,
                little_cluster,
                syscall,
                cpu_stats: mock_maker.make("cpu_stats_handler", Vec::new()),
                _mock_maker: mock_maker,
            }
        }

        let cluster_configs = make_default_cluster_configs();

        // Not allowed: Increasing dynamic power.
        let handlers = new_handlers_for_failed_validation();
        let thermal_state_configs = vec![
            ThermalStateConfig {
                cluster_opps: vec![0, 0],
                min_performance_normperfs: 0.0,
                static_power_w: 2.0,
                dynamic_power_per_normperf_w: 1.0,
            },
            ThermalStateConfig {
                cluster_opps: vec![2, 2],
                min_performance_normperfs: 0.0,
                static_power_w: 1.5,
                dynamic_power_per_normperf_w: 1.1,
            },
        ];
        let builder = CpuManagerMainBuilder::new(
            DEFAULT_SUSTAINABLE_POWER,
            DEFAULT_POWER_GAIN,
            cluster_configs.clone(),
            vec![handlers.big_cluster.clone(), handlers.little_cluster.clone()],
            thermal_state_configs.clone(),
            handlers.syscall.clone(),
            handlers.cpu_stats.clone(),
        );
        assert!(builder.build().unwrap().init().await.is_err());

        // Not allowed: Increasing performance capacity.
        let handlers = new_handlers_for_failed_validation();
        let thermal_state_configs = vec![
            ThermalStateConfig {
                cluster_opps: vec![2, 2],
                min_performance_normperfs: 0.0,
                static_power_w: 2.0,
                dynamic_power_per_normperf_w: 1.0,
            },
            ThermalStateConfig {
                cluster_opps: vec![2, 1],
                min_performance_normperfs: 0.0,
                static_power_w: 1.5,
                dynamic_power_per_normperf_w: 0.8,
            },
        ];
        let builder = CpuManagerMainBuilder::new(
            DEFAULT_SUSTAINABLE_POWER,
            DEFAULT_POWER_GAIN,
            cluster_configs.clone(),
            vec![handlers.big_cluster.clone(), handlers.little_cluster.clone()],
            thermal_state_configs,
            handlers.syscall.clone(),
            handlers.cpu_stats.clone(),
        );
        assert!(builder.build().unwrap().init().await.is_err());

        // Allowed: The second state has higher static power, but its power draw is less than the
        // first state at the first performance at which both states are admissible.
        let handlers = Handlers::new();
        let thermal_state_configs = vec![
            ThermalStateConfig {
                cluster_opps: vec![0, 0],
                min_performance_normperfs: 0.0,
                static_power_w: 2.0,
                dynamic_power_per_normperf_w: 1.0,
            },
            ThermalStateConfig {
                cluster_opps: vec![2, 2],
                min_performance_normperfs: 1.0,
                static_power_w: 2.1,
                dynamic_power_per_normperf_w: 0.8,
            },
        ];
        let builder = CpuManagerMainBuilder::new(
            DEFAULT_SUSTAINABLE_POWER,
            DEFAULT_POWER_GAIN,
            cluster_configs.clone(),
            vec![handlers.big_cluster.clone(), handlers.little_cluster.clone()],
            thermal_state_configs.clone(),
            handlers.syscall.clone(),
            handlers.cpu_stats.clone(),
        );
        builder.build_and_init().await;

        // Not allowed: The second state has higher power draw at the first performance at which
        // both states are admissible.
        let handlers = new_handlers_for_failed_validation();
        let thermal_state_configs = vec![
            ThermalStateConfig {
                cluster_opps: vec![0, 0],
                min_performance_normperfs: 0.0,
                static_power_w: 2.0,
                dynamic_power_per_normperf_w: 1.0,
            },
            ThermalStateConfig {
                cluster_opps: vec![2, 2],
                min_performance_normperfs: 1.0,
                static_power_w: 2.5,
                dynamic_power_per_normperf_w: 0.8,
            },
        ];
        let builder = CpuManagerMainBuilder::new(
            DEFAULT_SUSTAINABLE_POWER,
            DEFAULT_POWER_GAIN,
            cluster_configs.clone(),
            vec![handlers.big_cluster.clone(), handlers.little_cluster.clone()],
            thermal_state_configs.clone(),
            handlers.syscall.clone(),
            handlers.cpu_stats.clone(),
        );
        assert!(builder.build().unwrap().init().await.is_err());
    }

    // Tests that CpuManagerMainBuilder requires that clusters are configured to exactly span the space
    // of all logical CPU numbers.
    #[fuchsia::test]
    async fn test_validate_all_cpus_spanned() {
        let mut mock_maker = MockNodeMaker::new();

        // The big and little cluster handlers are initially queried for all operating points.
        let big_cluster_handler = create_dummy_node();
        let little_cluster_handler = create_dummy_node();

        // Configure the syscall handler to report one more CPU than is spanned by the clusters.
        let num_cpus = BIG_CPU_NUMBERS.len() + LITTLE_CPU_NUMBERS.len() + 1;
        let syscall_handler = mock_maker.make(
            "syscall_handler",
            vec![(msg_eq!(GetNumCpus), msg_ok_return!(GetNumCpus(num_cpus as u32)))],
        );

        let cpu_stats_handler = mock_maker.make("cpu_stats_handler", Vec::new());

        let thermal_state_configs = vec![ThermalStateConfig {
            cluster_opps: vec![0, 0],
            min_performance_normperfs: 0.0,
            static_power_w: 2.0,
            dynamic_power_per_normperf_w: 1.0,
        }];
        let builder = CpuManagerMainBuilder::new(
            DEFAULT_SUSTAINABLE_POWER,
            DEFAULT_POWER_GAIN,
            make_default_cluster_configs(),
            vec![big_cluster_handler, little_cluster_handler],
            thermal_state_configs,
            syscall_handler,
            cpu_stats_handler,
        );
        assert!(builder.build().unwrap().init().await.is_err());
    }

    // Verify that CpuManagerMain responds as expected to UpdateThermalLoad messages.
    #[fuchsia::test]
    async fn test_update_thermal_load() {
        let handlers = Handlers::new();

        let thermal_state_configs = vec![
            ThermalStateConfig {
                cluster_opps: vec![0, 0],
                min_performance_normperfs: 0.0,
                static_power_w: 2.0,
                dynamic_power_per_normperf_w: 1.0,
            },
            ThermalStateConfig {
                cluster_opps: vec![0, 1],
                min_performance_normperfs: 0.2,
                static_power_w: 1.5,
                dynamic_power_per_normperf_w: 0.8,
            },
            ThermalStateConfig {
                cluster_opps: vec![1, 2],
                min_performance_normperfs: 0.4,
                static_power_w: 1.0,
                dynamic_power_per_normperf_w: 0.6,
            },
        ];

        let node = CpuManagerMainBuilder::new(
            DEFAULT_SUSTAINABLE_POWER,
            DEFAULT_POWER_GAIN,
            make_default_cluster_configs(),
            vec![handlers.big_cluster.clone(), handlers.little_cluster.clone()],
            thermal_state_configs,
            handlers.syscall.clone(),
            handlers.cpu_stats.clone(),
        )
        .build_and_init()
        .await;

        // The thermal load is 52, we have max power consumption of 2.985W.
        // The current opp is 0, so with 0.1 fractional utililzation per core, we have:
        //   Big cluster: 0.2 cores load -> 0.4GHz utilized -> 0.4 NormPerfs
        //   Little cluster: 0.2 cores load -> 0.2GHz utilized -> 0.1 NormPerfs
        // At thermal state 0, the projected power use at 0.5 NormPerfs is
        //   2W static + 0.5W dynamic = 2.5W
        // This is within the 2.985W budget, so there are no opp changes.
        handlers.enqueue_cpu_loads(vec![0.1; 4]);
        let result = node.handle_message(&Message::UpdateThermalLoad(ThermalLoad(52))).await;
        result.unwrap();

        // The thermal load is 52, we have max power consumption of 2.985W.
        // The current opp is 0, so with 0.25 fractional utililzation per core, we have:
        //   Big cluster: 0.5 cores load -> 1GHz utilized -> 1 NormPerfs
        //   Little cluster: 0.5 cores load -> 0.5GHz utilized -> 0.25 NormPerfs
        // Projected power usage at 1.25 NormPerfs is:
        //   Thermal state 0: 2W static + 1.25 dynamic = 3.25W => over 2.985W budget
        //   Thermal state 1: 1.5W static + 1W dynamic = 2.5W => within 2.985W budget
        // So the new thermal state is 1, for which the little cluster changes to opp 1.
        handlers.enqueue_cpu_loads(vec![0.25; 4]);
        handlers.expect_little_opp(1);
        let result = node.handle_message(&Message::UpdateThermalLoad(ThermalLoad(52))).await;
        result.unwrap();

        // The thermal load is 61, we have max power consumption of 2.426W.
        // CPU load stays the same, but the power budget drops to 2.426W, below allocation for thermal
        // state 1. This pushes us to thermal state 2, with big opp 1 and little opp 2.
        handlers.enqueue_cpu_loads(vec![0.25; 4]);
        handlers.expect_big_opp(1);
        handlers.expect_little_opp(2);
        let result = node.handle_message(&Message::UpdateThermalLoad(ThermalLoad(61))).await;
        result.unwrap();

        // The thermal load is 77, we have max power consumption of 1.43W.
        // The power budget is 1.43W, which is below the static power for thermal state 1. However,
        // at 0.05 fractional utilization per core, the projected performance is 0.25 Perfs, which
        // makes thermal state 2 inadmissible. Thus, we fall back to thermal state 1.
        handlers.enqueue_cpu_loads(vec![0.05; 4]);
        handlers.expect_big_opp(0);
        handlers.expect_little_opp(1);
        let result = node.handle_message(&Message::UpdateThermalLoad(ThermalLoad(77))).await;
        result.unwrap();

        // The thermal load is 84, we have max power consumption of 0.9948W.
        // At 0.01 fractional utilization per core, the projected performance is 0.05 Perfs, so now
        // thermal state 1 is inadmissible. This drives us to thermal state 0.
        handlers.enqueue_cpu_loads(vec![0.01; 4]);
        handlers.expect_little_opp(0);
        let result = node.handle_message(&Message::UpdateThermalLoad(ThermalLoad(84))).await;
        result.unwrap();
    }

    // Verify that CPU saturation is handled as expected.
    #[fuchsia::test]
    async fn test_cpu_saturation() {
        let handlers = Handlers::new();

        let thermal_state_configs = vec![
            ThermalStateConfig {
                cluster_opps: vec![0, 0],
                min_performance_normperfs: 0.0,
                static_power_w: 2.0,
                dynamic_power_per_normperf_w: 1.0,
            },
            ThermalStateConfig {
                cluster_opps: vec![1, 1],
                min_performance_normperfs: 4.5,
                static_power_w: 1.5,
                dynamic_power_per_normperf_w: 0.8,
            },
            ThermalStateConfig {
                cluster_opps: vec![2, 2],
                min_performance_normperfs: 0.0,
                static_power_w: 1.0,
                dynamic_power_per_normperf_w: 0.6,
            },
        ];

        let node = CpuManagerMainBuilder::new(
            DEFAULT_SUSTAINABLE_POWER,
            DEFAULT_POWER_GAIN,
            make_default_cluster_configs(),
            vec![handlers.big_cluster.clone(), handlers.little_cluster.clone()],
            thermal_state_configs.clone(),
            handlers.syscall.clone(),
            handlers.cpu_stats.clone(),
        )
        .build_and_init()
        .await;

        // Start with low power to force thermal state 2.
        handlers.enqueue_cpu_loads(vec![0.1; 4]);
        handlers.expect_big_opp(2);
        handlers.expect_little_opp(2);
        let result = node.handle_message(&Message::UpdateThermalLoad(ThermalLoad(85))).await;
        result.unwrap();

        // Now saturate the CPUs. This corresponds to 4.4 NormPerfs.
        handlers.enqueue_cpu_loads(vec![1.0; 4]);

        // We choose a max power above state 1's max and below state 0's.
        let thermal_load = {
            let inner = node.mutable_inner.lock().await;
            let state = &inner.thermal_states[1];
            let state_1_max_power = state.estimate_power(Saturated);
            let max_power = state_1_max_power + Watts(0.1);
            assert_lt!(max_power, inner.thermal_states[0].estimate_power(Saturated));
            ThermalLoad(((DEFAULT_SUSTAINABLE_POWER.0 - max_power.0) / DEFAULT_POWER_GAIN.0) as u32)
        };

        // Now we confirm that:
        //  - State 1 is selected, verifying that its min performance was disregarded due to CPU
        //    saturation.
        handlers.expect_big_opp(1);
        handlers.expect_little_opp(1);
        let result = node.handle_message(&Message::UpdateThermalLoad(thermal_load)).await;
        result.unwrap();
    }

    // Verify that Inspect data is populated as expected.
    #[fuchsia::test]
    async fn test_inspect_data() {
        let handlers = Handlers::new();

        let thermal_states = vec![
            ThermalStateConfig {
                cluster_opps: vec![0, 0],
                min_performance_normperfs: 0.0,
                static_power_w: 2.0,
                dynamic_power_per_normperf_w: 1.0,
            },
            ThermalStateConfig {
                cluster_opps: vec![1, 2],
                min_performance_normperfs: 0.2,
                static_power_w: 1.5,
                dynamic_power_per_normperf_w: 0.8,
            },
        ];

        let inspector = inspect::Inspector::default();
        let builder = CpuManagerMainBuilder::new(
            DEFAULT_SUSTAINABLE_POWER,
            DEFAULT_POWER_GAIN,
            make_default_cluster_configs(),
            vec![handlers.big_cluster.clone(), handlers.little_cluster.clone()],
            thermal_states,
            handlers.syscall.clone(),
            handlers.cpu_stats.clone(),
        )
        .with_inspect_root(inspector.root());
        let node = builder.build_and_init().await;

        // The power budget of 1W exceeds the static power of thermal state 0, so we are pushed
        // to thermal state 1.
        handlers.enqueue_cpu_loads(vec![1.0; 4]);
        handlers.expect_big_opp(1);
        handlers.expect_little_opp(2);
        let result = node.handle_message(&Message::UpdateThermalLoad(ThermalLoad(84))).await;
        assert_matches!(result, Ok(_));

        let estimate =
            node.mutable_inner.lock().await.thermal_states[1].estimate_perf_and_power(Saturated);

        assert_data_tree!(
            inspector,
            root: {
                "CpuManagerMain": {
                    "state": {
                        "thermal_state_index": "1",
                        "last_performance (NormPerfs)": 5.0,
                        "boost_enabled": false,
                        "big_cluster": {
                            "last_load (#cores)": 2.0,
                        },
                        "little_cluster": {
                            "last_load (#cores)": 2.0,
                        },
                        "available_power (W)": 0.9948135999999996,
                        "projected_performance (NormPerfs)": estimate.performance.0,
                        "projected_power (W)": estimate.power.0,
                        "last_error": "<None>",
                    },
                    "thermal_states": {
                        "thermal_state_00": {
                            "cluster_opps": vec![0u64, 0],
                            "min_performance (NormPerfs)": 0.0,
                            "static_power (W)": 2.0,
                            "dynamic_power_per_normperf (W)": 1.0,
                            "performance_capacity (NormPerfs)": 5.0,
                        },
                        "thermal_state_01": {
                            "cluster_opps": vec![1u64, 2],
                            "min_performance (NormPerfs)": 0.2,
                            "static_power (W)": 1.5,
                            "dynamic_power_per_normperf (W)": 0.8,
                            "performance_capacity (NormPerfs)": 4.6,
                        },
                    }
                }
            }
        );
    }

    #[fuchsia::test]
    async fn test_set_boost() {
        let handlers = Handlers::new();

        let thermal_state_configs = vec![
            ThermalStateConfig {
                cluster_opps: vec![0, 0],
                min_performance_normperfs: 0.0,
                static_power_w: 1.0,
                dynamic_power_per_normperf_w: 0.5,
            },
            ThermalStateConfig {
                cluster_opps: vec![1, 1],
                min_performance_normperfs: 0.0,
                static_power_w: 0.5,
                dynamic_power_per_normperf_w: 0.1,
            },
        ];

        let node = CpuManagerMainBuilder::new(
            DEFAULT_SUSTAINABLE_POWER,
            DEFAULT_POWER_GAIN,
            make_default_cluster_configs(),
            vec![handlers.big_cluster.clone(), handlers.little_cluster.clone()],
            thermal_state_configs.clone(),
            handlers.syscall.clone(),
            handlers.cpu_stats.clone(),
        )
        .build_and_init()
        .await;

        // Enabling boost should take effect immediately
        handlers.big_cluster.add_msg_response_pair((
            msg_eq!(SetMinimumOperatingPointLimit(0)),
            msg_ok_return!(SetMinimumOperatingPointLimit),
        ));
        handlers.little_cluster.add_msg_response_pair((
            msg_eq!(SetMinimumOperatingPointLimit(0)),
            msg_ok_return!(SetMinimumOperatingPointLimit),
        ));
        let mut result = node.handle_message(&Message::SetBoost(true, 0)).await;
        result.unwrap();

        // Trigger thermal throttling.
        // available_power = 6.22 - 50 * 0.0622046 = 3.11W
        // state 0 power at saturation: 1.0 + 0.5 * 5.0 = 3.5W
        // state 1 power at saturation: 0.5 + 0.1 * 4.7 = 0.97W
        // Since state 0 power (3.5W) > available_power (3.11W), it should pick state 1.
        handlers.enqueue_cpu_loads(vec![1.0; 4]);
        handlers.expect_big_opp(1);
        handlers.expect_little_opp(1);
        node.handle_message(&Message::UpdateThermalLoad(ThermalLoad(50))).await.unwrap();

        // Disabling boost should also take effect immediately
        handlers.big_cluster.add_msg_response_pair((
            msg_eq!(SetMinimumOperatingPointLimit(2)),
            msg_ok_return!(SetMinimumOperatingPointLimit),
        ));
        handlers.little_cluster.add_msg_response_pair((
            msg_eq!(SetMinimumOperatingPointLimit(2)),
            msg_ok_return!(SetMinimumOperatingPointLimit),
        ));
        result = node.handle_message(&Message::SetBoost(false, 0)).await;
        result.unwrap();
    }

    #[fuchsia::test]
    async fn test_set_and_clear_max_frequency() {
        let handlers = Handlers::new();

        let thermal_state_configs = vec![ThermalStateConfig {
            cluster_opps: vec![0, 0],
            min_performance_normperfs: 0.0,
            static_power_w: 2.0,
            dynamic_power_per_normperf_w: 1.0,
        }];

        let node = CpuManagerMainBuilder::new(
            DEFAULT_SUSTAINABLE_POWER,
            DEFAULT_POWER_GAIN,
            make_default_cluster_configs(),
            vec![handlers.big_cluster.clone(), handlers.little_cluster.clone()],
            thermal_state_configs,
            handlers.syscall.clone(),
            handlers.cpu_stats.clone(),
        )
        .build_and_init()
        .await;

        handlers.expect_big_opp(1);
        node.handle_message(&Message::SetMaxFrequency(BIG_CLUSTER_INDEX as u64, Some(1)))
            .await
            .unwrap();

        // Now throttle the little cluster.
        handlers.expect_little_opp(2);
        node.handle_message(&Message::SetMaxFrequency(LITTLE_CLUSTER_INDEX as u64, Some(2)))
            .await
            .unwrap();

        // Now, clear the max frequency constraint on one cluster at a time.
        handlers.expect_big_opp(0);
        handlers.expect_little_opp(0);
        node.handle_message(&Message::SetMaxFrequency(BIG_CLUSTER_INDEX as u64, None))
            .await
            .unwrap();
        node.handle_message(&Message::SetMaxFrequency(LITTLE_CLUSTER_INDEX as u64, None))
            .await
            .unwrap();
    }

    #[fuchsia::test]
    async fn test_set_and_clear_max_frequency_is_affected_by_thermal_throttling() {
        let handlers = Handlers::new();

        let thermal_state_configs = vec![
            ThermalStateConfig {
                cluster_opps: vec![0, 0],
                min_performance_normperfs: 0.0,
                static_power_w: 2.0,
                dynamic_power_per_normperf_w: 1.0,
            },
            ThermalStateConfig {
                cluster_opps: vec![0, 1],
                min_performance_normperfs: 0.2,
                static_power_w: 1.5,
                dynamic_power_per_normperf_w: 0.8,
            },
            ThermalStateConfig {
                cluster_opps: vec![1, 2],
                min_performance_normperfs: 0.4,
                static_power_w: 1.0,
                dynamic_power_per_normperf_w: 0.6,
            },
        ];

        let node = CpuManagerMainBuilder::new(
            DEFAULT_SUSTAINABLE_POWER,
            DEFAULT_POWER_GAIN,
            make_default_cluster_configs(),
            vec![handlers.big_cluster.clone(), handlers.little_cluster.clone()],
            thermal_state_configs.clone(),
            handlers.syscall.clone(),
            handlers.cpu_stats.clone(),
        )
        .build_and_init()
        .await;

        // The thermal load is 61, we have max power consumption of 2.426W.
        // CPU load stays the same, but the power budget drops to 2.426W, below allocation for
        // thermal state 1. This pushes us to thermal state 2, with big opp 1 and little opp 2.
        handlers.enqueue_cpu_loads(vec![0.25; 4]);
        handlers.expect_big_opp(1);
        handlers.expect_little_opp(2);
        node.handle_message(&Message::UpdateThermalLoad(ThermalLoad(61))).await.unwrap();

        // Now cap the max frequency for both clusters, but only the big cluster
        // should change since the little cluster is already thermal throttled
        // to the max frequency.
        handlers.expect_big_opp(2);
        node.handle_message(&Message::SetMaxFrequency(BIG_CLUSTER_INDEX as u64, Some(2)))
            .await
            .unwrap();
        node.handle_message(&Message::SetMaxFrequency(LITTLE_CLUSTER_INDEX as u64, Some(2)))
            .await
            .unwrap();

        // And reset to power up to the thermal limit.
        // Big cluster should not change since it is still thermal throttled.
        handlers.expect_big_opp(1);
        node.handle_message(&Message::SetMaxFrequency(BIG_CLUSTER_INDEX as u64, None))
            .await
            .unwrap();
        node.handle_message(&Message::SetMaxFrequency(LITTLE_CLUSTER_INDEX as u64, None))
            .await
            .unwrap();

        // Reset thermal load.
        handlers.expect_big_opp(0);
        handlers.expect_little_opp(0);
        handlers.enqueue_cpu_loads(vec![0.0; 4]);
        node.handle_message(&Message::UpdateThermalLoad(ThermalLoad(0))).await.unwrap();

        // Now do the opposite: cap max frequency then thermal load.
        // Cap max frequency for both clusters to opp 2.
        handlers.expect_big_opp(2);
        handlers.expect_little_opp(2);
        node.handle_message(&Message::SetMaxFrequency(BIG_CLUSTER_INDEX as u64, Some(2)))
            .await
            .unwrap();
        node.handle_message(&Message::SetMaxFrequency(LITTLE_CLUSTER_INDEX as u64, Some(2)))
            .await
            .unwrap();

        // The thermal load is 52, we have max power consumption of 2.985W.
        // The current opp is 0, so with 0.25 fractional utililzation per core, we have:
        //   Big cluster: 0.5 cores load -> 1GHz utilized -> 1 NormPerfs
        //   Little cluster: 0.5 cores load -> 0.5GHz utilized -> 0.25 NormPerfs
        // Projected power usage at 1.25 NormPerfs is:
        //   Thermal state 0: 2W static + 1.25 dynamic = 3.25W => over 2.985W budget
        //   Thermal state 1: 1.5W static + 1W dynamic = 2.5W => within 2.985W budget
        // So the new thermal state is 1, for which the little cluster changes to opp 1;
        // however, the max frequency caps the big opp at 2 and the little opp at 2.
        handlers.enqueue_cpu_loads(vec![0.25; 4]);
        node.handle_message(&Message::UpdateThermalLoad(ThermalLoad(52))).await.unwrap();

        // Remove the limit. With the limit removed, thermal throttling is still in place.
        handlers.expect_big_opp(0);
        handlers.expect_little_opp(1);
        node.handle_message(&Message::SetMaxFrequency(BIG_CLUSTER_INDEX as u64, None))
            .await
            .unwrap();
        node.handle_message(&Message::SetMaxFrequency(LITTLE_CLUSTER_INDEX as u64, None))
            .await
            .unwrap();

        // Reset thermal load.
        handlers.expect_little_opp(0);
        handlers.enqueue_cpu_loads(vec![0.0; 4]);
        node.handle_message(&Message::UpdateThermalLoad(ThermalLoad(0))).await.unwrap();
    }

    #[fuchsia::test]
    async fn test_set_boost_multiple_sources() {
        let handlers = Handlers::new_with_highest_opp([1, 1]);

        let thermal_state_configs = vec![ThermalStateConfig {
            cluster_opps: vec![1, 1],
            min_performance_normperfs: 0.0,
            static_power_w: 1.5,
            dynamic_power_per_normperf_w: 0.0,
        }];

        let node = CpuManagerMainBuilder::new(
            DEFAULT_SUSTAINABLE_POWER,
            DEFAULT_POWER_GAIN,
            make_default_cluster_configs(),
            vec![handlers.big_cluster.clone(), handlers.little_cluster.clone()],
            thermal_state_configs.clone(),
            handlers.syscall.clone(),
            handlers.cpu_stats.clone(),
        )
        .build_and_init()
        .await;

        // 1. Enable boost with ID 0. Should set min limit to 0.
        handlers.big_cluster.add_msg_response_pair((
            msg_eq!(SetMinimumOperatingPointLimit(0)),
            msg_ok_return!(SetMinimumOperatingPointLimit),
        ));
        handlers.little_cluster.add_msg_response_pair((
            msg_eq!(SetMinimumOperatingPointLimit(0)),
            msg_ok_return!(SetMinimumOperatingPointLimit),
        ));
        let result = node.handle_message(&Message::SetBoost(true, 0)).await;
        result.unwrap();

        // 2. Enable boost with ID 1. Should be no-op (already enabled), but still track it.
        // No new messages expected.
        let result = node.handle_message(&Message::SetBoost(true, 1)).await;
        result.unwrap();

        // 3. Disable boost with ID 0. Should be no-op (still enabled by ID 1).
        // No new messages expected.
        let result = node.handle_message(&Message::SetBoost(false, 0)).await;
        result.unwrap();

        // 4. Disable boost with ID 1. Should set min limit back to 2 (disabled).
        handlers.big_cluster.add_msg_response_pair((
            msg_eq!(SetMinimumOperatingPointLimit(2)),
            msg_ok_return!(SetMinimumOperatingPointLimit),
        ));
        handlers.little_cluster.add_msg_response_pair((
            msg_eq!(SetMinimumOperatingPointLimit(2)),
            msg_ok_return!(SetMinimumOperatingPointLimit),
        ));
        let result = node.handle_message(&Message::SetBoost(false, 1)).await;
        result.unwrap();
    }

    #[fuchsia::test]
    async fn test_set_boost_idempotency_same_id() {
        let handlers = Handlers::new_with_highest_opp([1, 1]);

        let thermal_state_configs = vec![ThermalStateConfig {
            cluster_opps: vec![1, 1],
            min_performance_normperfs: 0.0,
            static_power_w: 1.5,
            dynamic_power_per_normperf_w: 0.0,
        }];

        let node = CpuManagerMainBuilder::new(
            DEFAULT_SUSTAINABLE_POWER,
            DEFAULT_POWER_GAIN,
            make_default_cluster_configs(),
            vec![handlers.big_cluster.clone(), handlers.little_cluster.clone()],
            thermal_state_configs.clone(),
            handlers.syscall.clone(),
            handlers.cpu_stats.clone(),
        )
        .build_and_init()
        .await;

        // 1. Enable boost with ID 0. Should set min limit to 0.
        handlers.big_cluster.add_msg_response_pair((
            msg_eq!(SetMinimumOperatingPointLimit(0)),
            msg_ok_return!(SetMinimumOperatingPointLimit),
        ));
        handlers.little_cluster.add_msg_response_pair((
            msg_eq!(SetMinimumOperatingPointLimit(0)),
            msg_ok_return!(SetMinimumOperatingPointLimit),
        ));
        let result = node.handle_message(&Message::SetBoost(true, 0)).await;
        result.unwrap();

        // 2. Enable boost with ID 0 again. Should be no-op.
        // No new messages expected.
        let result = node.handle_message(&Message::SetBoost(true, 0)).await;
        result.unwrap();

        // 3. Disable boost with ID 0. Should set min limit back to 2 (disabled).
        handlers.big_cluster.add_msg_response_pair((
            msg_eq!(SetMinimumOperatingPointLimit(2)),
            msg_ok_return!(SetMinimumOperatingPointLimit),
        ));
        handlers.little_cluster.add_msg_response_pair((
            msg_eq!(SetMinimumOperatingPointLimit(2)),
            msg_ok_return!(SetMinimumOperatingPointLimit),
        ));
        let result = node.handle_message(&Message::SetBoost(false, 0)).await;
        result.unwrap();
    }
}
