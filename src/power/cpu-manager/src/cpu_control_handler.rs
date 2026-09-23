// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::CpuManagerError;
use crate::message::{Message, MessageReturn};
use crate::node::Node;
use crate::ok_or_default_err;
use crate::types::{Farads, Hertz, OperatingPoint, ThermalLoad, Volts, Watts};
use crate::utils::get_cpu_ctrl_proxy;
use anyhow::{Context, Error, format_err};
use async_trait::async_trait;
use async_utils::event::Event as AsyncEvent;
use fidl_fuchsia_hardware_cpu_ctrl as fcpuctrl;
use fidl_fuchsia_thermal as fthermal;
use fuchsia_inspect::{self as inspect, Property};
use serde_derive::Deserialize;
use serde_json as json;
use std::cell::{Ref, RefCell};
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

/// Node: CpuControlHandler
///
/// Summary: Provides a mechanism for controlling the operating point of a CPU domain. The node
///          mostly relies on functionality provided by the DeviceControlHandler node for
///          operating point control, but this node enhances that basic functionality by
///          integrating operating point metadata (CPU opp information) from the CpuCtrl
///          interface.
///
/// Handles Messages:
///     - UpdateThermalLoad
///
/// Sends Messages:
///     - GetCpuLoads
///     - GetOperatingPoint
///     - SetOperatingPoint
///
/// FIDL dependencies:
///     - fuchsia.hardware.cpu.ctrl.Device: the node uses this protocol to communicate with the
///       CpuCtrl interface of the CPU device specified in the CpuControlHandler constructor

/// Describes the parameters of the CPU domain.
pub struct CpuControlParams {
    /// Available opps of the CPU. These must be in order of descending power usage, per
    /// section 8.4.6.2 of ACPI spec version 6.3.
    pub opps: Vec<OperatingPoint>,
    /// Model capacitance of each CPU core. Required to estimate power usage.
    pub capacitance: Farads,
    /// Logical CPU numbers contained within this CPU domain.
    pub logical_cpu_numbers: Vec<u32>,
}

impl CpuControlParams {
    /// Checks that the list of opps is valid:
    ///  - Contains at least one element;
    ///  - Is in order of decreasing nominal power consumption.
    fn validate(&self) -> Result<(), Error> {
        if self.logical_cpu_numbers.len() == 0 {
            return Err(format_err!("Must have > 0 CPUs"));
        }
        if !self.logical_cpu_numbers.array_windows().all(|&[a, b]| a < b) {
            return Err(format_err!("CPUs must be sorted and non-repeating"));
        }
        if self.opps.len() == 0 {
            return Err(format_err!("Must have at least one opp"));
        } else if self.opps.len() > 1 {
            let mut last_power =
                get_cpu_power(self.capacitance, self.opps[0].voltage, self.opps[0].frequency);
            for i in 1..self.opps.len() {
                let opp = &self.opps[i];
                let power = get_cpu_power(self.capacitance, opp.voltage, opp.frequency);
                if power >= last_power {
                    return Err(format_err!(
                        "opps must be in order of decreasing power consumption \
                         (violated by state {})",
                        i
                    ));
                }
                last_power = power;
            }
        }
        Ok(())
    }
}

/// Returns the modeled power consumed by the CPU completing operations at the specified rate.
/// Note that we assume zero static power consumption in this model, and that `op_completion_rate`
/// is only the CPU's clock speed if the CPU is never idle.
pub fn get_cpu_power(capacitance: Farads, voltage: Volts, op_completion_rate: Hertz) -> Watts {
    Watts(capacitance.0 * voltage.0.powi(2) * op_completion_rate.0)
}

/// A builder for constructing the CpuControlHandler node. The fields of this struct are documented
/// as part of the CpuControlHandler struct.
pub struct CpuControlHandlerBuilder<'a> {
    sustainable_power: Option<Watts>,
    power_gain: Option<Watts>,
    total_domain_count: Option<u8>,
    perf_rank: Option<u8>,
    cpu_stats_handler: Option<Rc<dyn Node>>,
    cpu_dev_handler: Option<Rc<dyn Node>>,
    cpu_ctrl_proxy: Option<fcpuctrl::DeviceProxy>,
    inspect_root: Option<&'a inspect::Node>,
    min_cpu_clock_speed: Option<Hertz>,
    capacitance: Option<Farads>,
    logical_cpu_numbers: Option<Vec<u32>>,
}

impl<'a> CpuControlHandlerBuilder<'a> {
    #[cfg(test)]
    pub fn new() -> Self {
        use crate::test::mock_node::create_dummy_node;

        Self {
            sustainable_power: Some(Watts(0.0)),
            power_gain: Some(Watts(0.0)),
            total_domain_count: Some(1),
            perf_rank: Some(0),
            cpu_stats_handler: Some(create_dummy_node()),
            cpu_dev_handler: Some(create_dummy_node()),
            cpu_ctrl_proxy: None,
            inspect_root: None,
            min_cpu_clock_speed: Some(Hertz(0.0)),
            capacitance: Some(Farads(0.0)),
            logical_cpu_numbers: Some(vec![0]),
        }
    }

    #[cfg(test)]
    pub fn sustainable_power(mut self, power: Watts) -> Self {
        self.sustainable_power = Some(power);
        self
    }

    #[cfg(test)]
    pub fn power_gain(mut self, power: Watts) -> Self {
        self.power_gain = Some(power);
        self
    }

    #[cfg(test)]
    pub fn cpu_stats_handler(mut self, handler: Rc<dyn Node>) -> Self {
        self.cpu_stats_handler = Some(handler);
        self
    }

    #[cfg(test)]
    pub fn cpu_dev_handler(mut self, handler: Rc<dyn Node>) -> Self {
        self.cpu_dev_handler = Some(handler);
        self
    }

    #[cfg(test)]
    pub fn cpu_ctrl_proxy(mut self, proxy: fcpuctrl::DeviceProxy) -> Self {
        self.cpu_ctrl_proxy = Some(proxy);
        self
    }

    #[cfg(test)]
    pub fn capacitance(mut self, capacitance: Farads) -> Self {
        self.capacitance = Some(capacitance);
        self
    }

    #[cfg(test)]
    fn inspect_root(mut self, inspect_root: &'a inspect::Node) -> Self {
        self.inspect_root = Some(inspect_root);
        self
    }

    #[cfg(test)]
    pub fn logical_cpu_numbers(mut self, cpu_numbers: Vec<u32>) -> Self {
        self.logical_cpu_numbers = Some(cpu_numbers);
        self
    }

    #[cfg(test)]
    fn min_cpu_clock_speed(mut self, clock_speed: Hertz) -> Self {
        self.min_cpu_clock_speed = Some(clock_speed);
        self
    }

    pub fn new_from_json(json_data: json::Value, nodes: &HashMap<String, Rc<dyn Node>>) -> Self {
        #[derive(Deserialize)]
        struct Config {
            sustainable_power: f64,
            power_gain: f64,
            total_domain_count: u8,
            perf_rank: u8,
            capacitance: f64,
            min_cpu_clock_speed: f64,
            logical_cpu_numbers: Vec<u32>,
        }

        #[derive(Deserialize)]
        struct Dependencies {
            cpu_stats_handler_node: String,
            cpu_dev_handler_node: String,
        }

        #[derive(Deserialize)]
        struct JsonData {
            config: Config,
            dependencies: Dependencies,
        }

        let data: JsonData = json::from_value(json_data).unwrap();
        Self {
            sustainable_power: Some(Watts(data.config.sustainable_power)),
            power_gain: Some(Watts(data.config.power_gain)),
            total_domain_count: Some(data.config.total_domain_count),
            perf_rank: Some(data.config.perf_rank),
            min_cpu_clock_speed: Some(Hertz(data.config.min_cpu_clock_speed)),
            cpu_stats_handler: Some(nodes[&data.dependencies.cpu_stats_handler_node].clone()),
            cpu_dev_handler: Some(nodes[&data.dependencies.cpu_dev_handler_node].clone()),
            capacitance: Some(Farads(data.config.capacitance)),
            logical_cpu_numbers: Some(data.config.logical_cpu_numbers),
            cpu_ctrl_proxy: None,
            inspect_root: None,
        }
    }

    pub fn build(self) -> Result<Rc<CpuControlHandler>, Error> {
        let sustainable_power = ok_or_default_err!(self.sustainable_power)?;
        let power_gain = ok_or_default_err!(self.power_gain)?;
        let total_domain_count = ok_or_default_err!(self.total_domain_count)?;
        let perf_rank = ok_or_default_err!(self.perf_rank)?;
        let cpu_stats_handler = ok_or_default_err!(self.cpu_stats_handler)?;
        let cpu_dev_handler = ok_or_default_err!(self.cpu_dev_handler)?;
        let min_cpu_clock_speed = ok_or_default_err!(self.min_cpu_clock_speed)?;
        let capacitance = ok_or_default_err!(self.capacitance)?;
        let logical_cpu_numbers = ok_or_default_err!(self.logical_cpu_numbers)?;

        // Optionally use the default inspect root node
        let inspect_root =
            self.inspect_root.unwrap_or_else(|| inspect::component::inspector().root());
        let inspect =
            InspectData::new(inspect_root, format!("CpuControlHandler (perf_rank: {})", perf_rank));

        let mutable_inner = MutableInner {
            current_opp_index: 0,
            cpu_control_params: CpuControlParams {
                opps: Vec::new(),
                capacitance,
                logical_cpu_numbers,
            },
            cpu_ctrl_proxy: self.cpu_ctrl_proxy,
        };

        let mut hasher = DefaultHasher::new();
        perf_rank.hash(&mut hasher);
        let trace_counter_id = hasher.finish();

        Ok(Rc::new(CpuControlHandler {
            sustainable_power,
            power_gain,
            init_done: AsyncEvent::new(),
            total_domain_count,
            perf_rank,
            cpu_stats_handler,
            cpu_dev_handler,
            inspect,
            trace_counter_id,
            min_cpu_clock_speed,
            mutable_inner: RefCell::new(mutable_inner),
        }))
    }

    #[cfg(test)]
    pub async fn build_and_init(self) -> Rc<CpuControlHandler> {
        let node = self.build().unwrap();
        node.init().await.unwrap();
        node
    }
}

pub struct CpuControlHandler {
    /// The available power when thermal load is 0.
    sustainable_power: Watts,

    /// A scale factor that maps thermal load to available power.
    power_gain: Watts,

    /// Signalled after `init()` has completed. Used to ensure node doesn't process messages until
    /// its `init()` has completed.
    init_done: AsyncEvent,

    /// Total number of CPU devices.
    total_domain_count: u8,

    /// Performance rank of the CPU device, where rank is the position in a list of all CPU devices
    /// sorted by relative performance from highest to lowest.
    perf_rank: u8,

    /// The node which will provide CPU load information. It is expected that this node responds to
    /// the GetCpuLoads message.
    cpu_stats_handler: Rc<dyn Node>,

    /// The node to be used for CPU operating point control. It is expected that this node
    /// responds to the Get/SetOperatingPoint messages.
    cpu_dev_handler: Rc<dyn Node>,

    /// A struct for managing Component Inspection data
    inspect: InspectData,

    /// Identifies trace counters between CpuControlHandler instances for different drivers.
    trace_counter_id: u64,

    /// Minimum CPU clock speed to set. Selectable CPU opps with a frequency below this value
    /// are filtered out.
    min_cpu_clock_speed: Hertz,

    /// Mutable inner state.
    mutable_inner: RefCell<MutableInner>,
}

impl CpuControlHandler {
    /// Convenience accessor for borrowing `cpu_control_params`.
    fn cpu_control_params(&self) -> Ref<'_, CpuControlParams> {
        Ref::map(self.mutable_inner.borrow(), |inner| &inner.cpu_control_params)
    }

    /// Returns the total CPU load (averaged since the previous call)
    async fn get_load(&self) -> Result<f32, Error> {
        fuchsia_trace::duration!(
            c"cpu_manager",
            c"CpuControlHandler::get_load",
            "perf_rank" => self.perf_rank as u32
        );

        // Get load for all CPUs in the system
        let cpu_loads =
            match self.send_message(&self.cpu_stats_handler, &Message::GetCpuLoads).await {
                Ok(MessageReturn::GetCpuLoads(loads)) => Ok(loads),
                Ok(r) => Err(format_err!("GetCpuLoads had unexpected return value: {:?}", r)),
                Err(e) => Err(format_err!("GetCpuLoads failed: {:?}", e)),
            }?;

        // Filter down to only the ones we're concerned with
        Ok(self
            .cpu_control_params()
            .logical_cpu_numbers
            .iter()
            .map(|i| cpu_loads[*i as usize])
            .sum())
    }

    /// Returns the current CPU opp index
    async fn get_current_opp_index(&self) -> Result<usize, Error> {
        fuchsia_trace::duration!(
            c"cpu_manager",
            c"CpuControlHandler::get_current_opp_index",
            "perf_rank" => self.perf_rank as u32
        );
        match self.send_message(&self.cpu_dev_handler, &Message::GetOperatingPoint).await {
            Ok(MessageReturn::GetOperatingPoint(state)) => Ok(state as usize),
            Ok(r) => Err(format_err!("GetOperatingPoint had unexpected return value: {:?}", r)),
            Err(e) => Err(format_err!("GetOperatingPoint failed: {:?}", e)),
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
            c"CpuControlHandler::handle_update_thermal_load",
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
            c"CpuControlHandler available_power",
            0,
            "available_power" => available_power.0
        );

        if let Err(e) = self.set_max_power_consumption(&available_power).await {
            log::error!("Error setting max power consumption: {}", e);
        }

        Ok(MessageReturn::UpdateThermalLoad)
    }

    /// Calculate available power based on thermal load.
    fn calculate_available_power(&self, thermal_load: ThermalLoad) -> Watts {
        fuchsia_trace::duration!(
            c"cpu_manager",
            c"CpuControlHandler::calculate_available_power",
            "thermal_load" => thermal_load.0
        );

        let power_available =
            f64::max(0.0, self.sustainable_power.0 - thermal_load.0 as f64 * self.power_gain.0);

        Watts(power_available)
    }

    /// Sets the opp to the highest-power state with consumption below `max_power`.
    ///
    /// The estimated power consumption depends on the operation completion rate by the CPU.
    /// We assume that the rate of operations requested over the next sample interval will be
    /// the same as it was over the previous sample interval, up to CPU's max completion rate
    /// for a opp under consideration.
    async fn set_max_power_consumption(&self, max_power: &Watts) -> Result<(), Error> {
        fuchsia_trace::duration!(
            c"cpu_manager",
            c"CpuControlHandler::set_max_power_consumption",
            "perf_rank" => self.perf_rank as u32,
            "max_power" => max_power.0
        );

        self.init_done.wait().await;

        let current_opp_index = self.mutable_inner.borrow().current_opp_index;

        // This is reused several times.
        let num_cores = self.cpu_control_params().logical_cpu_numbers.len() as f64;

        // The operation completion rate over the last sample interval is
        //     num_operations / sample_interval,
        // where
        //     num_operations = last_load * last_frequency * sample_interval.
        // Hence,
        //     last_op_rate = last_load * last_frequency.
        let (last_op_rate, last_max_op_rate) = {
            let last_load = self.get_load().await? as f64;
            self.inspect.last_load.set(last_load);
            fuchsia_trace::counter!(
                c"cpu_manager",
                c"CpuControlHandler last_load",
                self.trace_counter_id,
                &self.perf_rank.to_string() => last_load
            );

            let last_frequency = self.cpu_control_params().opps[current_opp_index].frequency;
            (last_frequency.mul_scalar(last_load), last_frequency.mul_scalar(num_cores))
        };

        self.inspect.last_op_rate.set(last_op_rate.0);
        fuchsia_trace::instant!(
            c"cpu_manager",
            c"CpuControlHandler::set_max_power_consumption_data",
            fuchsia_trace::Scope::Thread,
            "perf_rank" => self.perf_rank as u32,
            "current_opp_index" => current_opp_index as u32,
            "last_op_rate" => last_op_rate.0
        );

        let mut opp_index = 0;
        let mut estimated_power = Watts(0.0);

        // Iterate through the list of available opps (guaranteed to be sorted in order of
        // decreasing power consumption) and choose the first that will operate within the
        // `max_power` constraint.
        for (i, state) in self.cpu_control_params().opps.iter().enumerate() {
            // We assume that the last operation rate carries over to the next interval unless:
            //  - It exceeds the max operation rate at the new frequency, in which case it is
            //    truncated to the new max.
            //  - It is within a small delta of the max rate at the last frequency, in which case we
            //    assume that it would rise to the new maximum if the clock speed were to increase.
            const ESSENTIALLY_MAX_LOAD_FRACTION: f64 = 0.99;
            let new_max_op_rate = state.frequency.mul_scalar(num_cores);
            let estimated_op_rate = if last_op_rate > new_max_op_rate
                || last_op_rate > last_max_op_rate.mul_scalar(ESSENTIALLY_MAX_LOAD_FRACTION)
            {
                new_max_op_rate
            } else {
                last_op_rate
            };

            opp_index = i;
            estimated_power = get_cpu_power(
                self.cpu_control_params().capacitance,
                state.voltage,
                estimated_op_rate,
            );

            if estimated_power <= *max_power {
                break;
            }
        }

        fuchsia_trace::counter!(
            c"cpu_manager",
            c"CpuControlHandler estimated_power",
            0,
            "value (W)" => estimated_power.0
        );

        if opp_index != current_opp_index {
            fuchsia_trace::instant!(
                c"cpu_manager",
                c"CpuControlHandler::updated_opp_index",
                fuchsia_trace::Scope::Thread,
                "perf_rank" => self.perf_rank as u32,
                "old_index" => current_opp_index as u32,
                "new_index" => opp_index as u32
            );

            // Tell the CPU DeviceControlHandler to update the operating point
            self.send_message(&self.cpu_dev_handler, &Message::SetOperatingPoint(opp_index as u32))
                .await?;

            // Cache the new opp index for calculations on the next iteration
            self.mutable_inner.borrow_mut().current_opp_index = opp_index;
            self.inspect.opp_index.set(opp_index as u64);
        }

        fuchsia_trace::counter!(
            c"cpu_manager",
            c"CpuControlHandler opp",
            self.trace_counter_id,
            &self.perf_rank.to_string() => opp_index as u32
        );
        Ok(())
    }
}

struct MutableInner {
    /// The parameters of the CPU domain which are queried from the CPU driver.
    cpu_control_params: CpuControlParams,

    /// The current CPU opp index which is queried from the CPU DeviceControlHandler node.
    current_opp_index: usize,

    /// A proxy handle to communicate with the CPU driver CpuCtrl interface.
    cpu_ctrl_proxy: Option<fcpuctrl::DeviceProxy>,
}

#[async_trait(?Send)]
impl Node for CpuControlHandler {
    fn name(&self) -> String {
        format!("CpuControlHandler (perf_rank: {})", self.perf_rank)
    }

    /// Initializes internal state.
    ///
    /// Connects to the cpu-ctrl driver unless a proxy was already provided (in a test).
    async fn init(&self) -> Result<(), Error> {
        fuchsia_trace::duration!("cpu_manager", "CpuControlHandler::init");

        // Connect to the cpu-ctrl driver. Typically this is None, but it may be set by tests.
        let cpu_ctrl_proxy = match &self.mutable_inner.borrow().cpu_ctrl_proxy {
            Some(p) => p.clone(),
            None => {
                get_cpu_ctrl_proxy(&self.name(), self.total_domain_count, self.perf_rank).await?
            }
        };

        // Query the CPU opps
        let opps = get_opps(self.perf_rank, &cpu_ctrl_proxy, self.min_cpu_clock_speed)
            .await
            .context("Failed to get CPU opps")?;

        let current_opp = self.get_current_opp_index().await?;

        {
            let mut mutable_inner = self.mutable_inner.borrow_mut();
            let cpu_control_params = &mut mutable_inner.cpu_control_params;
            cpu_control_params.opps = opps;
            cpu_control_params.validate().context("Invalid CPU control params")?;
            self.inspect.set_cpu_control_params(&cpu_control_params);

            mutable_inner.cpu_ctrl_proxy = Some(cpu_ctrl_proxy);
            mutable_inner.current_opp_index = current_opp;
        }

        self.init_done.signal();

        Ok(())
    }

    async fn handle_message(&self, msg: &Message) -> Result<MessageReturn, CpuManagerError> {
        match msg {
            Message::UpdateThermalLoad(thermal_load) => {
                self.handle_update_thermal_load(*thermal_load).await
            }
            _ => Err(CpuManagerError::Unsupported),
        }
    }
}

struct InspectData {
    // Nodes
    root_node: inspect::Node,

    // Properties
    opp_index: inspect::UintProperty,
    last_op_rate: inspect::DoubleProperty,
    last_load: inspect::DoubleProperty,
}

impl InspectData {
    fn new(parent: &inspect::Node, node_name: String) -> Self {
        // Create a local root node and properties
        let root_node = parent.create_child(node_name);
        let opp_index = root_node.create_uint("opp_index", 0);
        let last_op_rate = root_node.create_double("last_op_rate", 0.0);
        let last_load = root_node.create_double("last_load", 0.0);

        InspectData { root_node, opp_index, last_op_rate, last_load }
    }

    fn set_cpu_control_params(&self, params: &CpuControlParams) {
        let cpu_params_node = self.root_node.create_child("cpu_control_params");

        // Iterate `params.opps` in reverse order so that the Inspect nodes appear in the same
        // order as the vector (`create_child` inserts nodes at the head).
        for (i, opp) in params.opps.iter().enumerate().rev() {
            let opp_node = cpu_params_node.create_child(format!("opp_{}", i));
            opp_node.record_double("voltage (V)", opp.voltage.0);
            opp_node.record_double("frequency (Hz)", opp.frequency.0);

            // Pass ownership of the new opp node to the parent `cpu_params_node`
            cpu_params_node.record(opp_node);
        }

        cpu_params_node.record_double("capacitance (F)", params.capacitance.0);
        cpu_params_node.record_string(
            "logical_cpu_numbers",
            format!("{:?}", params.logical_cpu_numbers).as_str(),
        );

        // Pass ownership of the new `cpu_params_node` to the root node
        self.root_node.record(cpu_params_node);
    }
}

/// Query the CPU opps from the CpuCtrl driver.
async fn get_opps(
    perf_rank: u8,
    cpu_ctrl_proxy: &fcpuctrl::DeviceProxy,
    min_cpu_clock_speed: Hertz,
) -> Result<Vec<OperatingPoint>, Error> {
    fuchsia_trace::duration!(
        c"cpu_manager",
        c"cpu_control_handler::get_opps",
        "perf_rank" => perf_rank as u32
    );

    // Query opp metadata from the CpuCtrl interface. Each supported operating point has
    // accompanying opp metadata.
    let mut opps = Vec::new();
    let mut skipped_opps = Vec::new();
    let opp_count = cpu_ctrl_proxy
        .get_operating_point_count()
        .await
        .map_err(|e| {
            format_err!(
                "CPU driver (perf_rank: {}): get_operating_point_count IPC failed: {}",
                perf_rank,
                e
            )
        })?
        .map_err(|e| {
            format_err!(
                "CPU driver (perf_rank: {}): get_operating_point_count returned error: {}",
                perf_rank,
                e
            )
        })?;

    for i in 0..opp_count {
        let info = cpu_ctrl_proxy
            .get_operating_point_info(i)
            .await
            .map_err(|e| {
                format_err!(
                    "CPU driver (perf_rank: {}): get_operating_point_info IPC failed: {}",
                    perf_rank,
                    e
                )
            })?
            .map_err(|e| {
                format_err!(
                    "CPU driver (perf_rank: {}): get_operating_point_info returned error: {}",
                    perf_rank,
                    e
                )
            })?;

        let frequency = Hertz(info.frequency_hz as f64);
        let voltage = Volts(info.voltage_uv as f64 / 1e6);
        let opp = OperatingPoint { frequency, voltage };

        // Filter out opps where CPU frequency is unacceptably low
        if frequency >= min_cpu_clock_speed {
            opps.push(opp);
        } else {
            skipped_opps.push(opp);
        }
    }

    fuchsia_trace::instant!(
        c"cpu_manager",
        c"cpu_control_handler::received_cpu_opps",
        fuchsia_trace::Scope::Thread,
        "perf_rank" => perf_rank as u32,
        "valid" => 1,
        "opps" => format!("{:?}", opps).as_str(),
        "skipped_opps" => format!("{:?}", skipped_opps).as_str()
    );

    Ok(opps)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::test::mock_node::{MessageMatcher, MockNodeMaker};
    use crate::{msg_eq, msg_ok_return};
    use assert_matches::assert_matches;
    use diagnostics_assertions::assert_data_tree;
    use fuchsia_async as fasync;
    use futures::TryStreamExt;
    use std::collections::HashSet;

    // Returns a proxy to a fake CpuCtrl driver pre-baked to return a single (fake) CPU opp.
    fn fake_cpu_ctrl_driver() -> fcpuctrl::DeviceProxy {
        fake_cpu_ctrl_driver_with_opps(vec![OperatingPoint {
            frequency: Hertz(0.0),
            voltage: Volts(0.0),
        }])
    }

    // Returns a proxy to a fake CpuCtrl driver pre-baked to return the given set of CPU opps.
    pub fn fake_cpu_ctrl_driver_with_opps(opps: Vec<OperatingPoint>) -> fcpuctrl::DeviceProxy {
        let (proxy, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<fcpuctrl::DeviceMarker>();

        fasync::Task::local(async move {
            while let Ok(req) = stream.try_next().await {
                match req {
                    Some(fcpuctrl::DeviceRequest::GetOperatingPointInfo { opp, responder }) => {
                        let index = opp as usize;
                        let result = if index < opps.len() {
                            Ok(fcpuctrl::CpuOperatingPointInfo {
                                frequency_hz: opps[index].frequency.0 as i64,
                                voltage_uv: (opps[index].voltage.0 * 1e6) as i64,
                            })
                        } else {
                            Err(zx::Status::NOT_SUPPORTED.into_raw())
                        };
                        let _ = responder.send(result.as_ref().map_err(|e| *e));
                    }
                    Some(fcpuctrl::DeviceRequest::GetOperatingPointCount { responder }) => {
                        let _ = responder.send(Ok(opps.len() as u32));
                    }
                    _ => assert!(false),
                }
            }
        })
        .detach();

        proxy
    }

    #[fuchsia::test]
    fn test_get_cpu_power() {
        assert_eq!(get_cpu_power(Farads(100.0e-12), Volts(1.0), Hertz(1.0e9)), Watts(0.1));
    }

    /// Tests that an unsupported message is handled gracefully and an Unsupported error is returned
    #[fuchsia::test]
    async fn test_unsupported_msg() {
        let mut mock_maker = MockNodeMaker::new();
        let devhost_node = mock_maker.make(
            "DevHostNode",
            vec![
                // CpuControlHandler queries operating point during its initialization
                (msg_eq!(GetOperatingPoint), msg_ok_return!(GetOperatingPoint(0))),
            ],
        );

        let cpu_ctrl_node = CpuControlHandlerBuilder::new()
            .cpu_ctrl_proxy(fake_cpu_ctrl_driver())
            .cpu_dev_handler(devhost_node)
            .build_and_init()
            .await;

        assert_matches!(
            cpu_ctrl_node.handle_message(&Message::GetCpuLoads).await,
            Err(CpuManagerError::Unsupported)
        );
    }

    /// Tests that CpuControlParams' `validate` correctly returns an error under invalid inputs.
    #[fuchsia::test]
    fn test_invalid_cpu_params() {
        // Empty CPUs
        assert!(
            CpuControlParams {
                logical_cpu_numbers: vec![],
                opps: vec![OperatingPoint { frequency: Hertz(0.0), voltage: Volts(0.0) }],
                capacitance: Farads(100e-12),
            }
            .validate()
            .is_err()
        );

        // Repeating CPUs
        assert!(
            CpuControlParams {
                logical_cpu_numbers: vec![0, 0],
                opps: vec![OperatingPoint { frequency: Hertz(0.0), voltage: Volts(0.0) }],
                capacitance: Farads(100e-12)
            }
            .validate()
            .is_err()
        );

        // Non-ascending CPUs
        assert!(
            CpuControlParams {
                logical_cpu_numbers: vec![1, 0],
                opps: vec![OperatingPoint { frequency: Hertz(0.0), voltage: Volts(0.0) }],
                capacitance: Farads(100e-12)
            }
            .validate()
            .is_err()
        );

        // Empty opps
        assert!(
            CpuControlParams {
                logical_cpu_numbers: vec![0],
                opps: vec![],
                capacitance: Farads(100e-12)
            }
            .validate()
            .is_err()
        );

        // opps in order of increasing power usage
        assert!(
            CpuControlParams {
                logical_cpu_numbers: vec![0],
                opps: vec![
                    OperatingPoint { frequency: Hertz(1.0), voltage: Volts(1.0) },
                    OperatingPoint { frequency: Hertz(2.0), voltage: Volts(1.0) }
                ],
                capacitance: Farads(100e-12)
            }
            .validate()
            .is_err()
        );

        // opps with identical power usage
        assert!(
            CpuControlParams {
                logical_cpu_numbers: vec![0],
                opps: vec![
                    OperatingPoint { frequency: Hertz(1.0), voltage: Volts(1.0) },
                    OperatingPoint { frequency: Hertz(1.0), voltage: Volts(1.0) }
                ],
                capacitance: Farads(100e-12)
            }
            .validate()
            .is_err()
        );
    }

    async fn get_operating_point(devhost_node: Rc<dyn Node>) -> u32 {
        match devhost_node.handle_message(&Message::GetOperatingPoint).await.unwrap() {
            MessageReturn::GetOperatingPoint(state) => state,
            e => panic!("Unexpected return value: {:?}", e),
        }
    }

    /// Tests that the UpdateThermalLoad message causes the node to correctly consider CPU load
    /// and parameters to choose the appropriate opps.
    #[fuchsia::test]
    async fn test_update_thermal_load() {
        let mut mock_maker = MockNodeMaker::new();

        // Arbitrary CpuControlParams chosen to allow the node to demonstrate opp selection
        let cpu_params = CpuControlParams {
            logical_cpu_numbers: vec![0, 1, 2, 3],
            opps: vec![
                OperatingPoint { frequency: Hertz(2.0e9), voltage: Volts(5.0) },
                OperatingPoint { frequency: Hertz(2.0e9), voltage: Volts(4.0) },
                OperatingPoint { frequency: Hertz(2.0e9), voltage: Volts(3.0) },
            ],
            capacitance: Farads(100.0e-12),
        };

        // The modeled power consumption at each opp
        let power_consumption: Vec<Watts> = cpu_params
            .opps
            .iter()
            .map(|opp| {
                get_cpu_power(
                    cpu_params.capacitance,
                    opp.voltage,
                    opp.frequency.mul_scalar(cpu_params.logical_cpu_numbers.len() as f64),
                )
            })
            .collect();

        let stats_node = mock_maker.make(
            "StatsNode",
            // The CpuControlHandler node queries the current CPU load each time it receives a
            // UpdateThermalLoad message
            vec![
                // Make StatsNode give load for more CPUs than we care about to test the filtering
                // logic
                (msg_eq!(GetCpuLoads), msg_ok_return!(GetCpuLoads(vec![1.0; 8]))),
                (msg_eq!(GetCpuLoads), msg_ok_return!(GetCpuLoads(vec![1.0; 8]))),
                (msg_eq!(GetCpuLoads), msg_ok_return!(GetCpuLoads(vec![1.0; 8]))),
            ],
        );
        let devhost_node = mock_maker.make(
            "DevHostNode",
            vec![
                // CpuControlHandler queries operating point during its initialization
                (msg_eq!(GetOperatingPoint), msg_ok_return!(GetOperatingPoint(0))),
                // The test queries for current operating point
                (msg_eq!(GetOperatingPoint), msg_ok_return!(GetOperatingPoint(0))),
                // CpuControlHandler changes operating point to 1
                (msg_eq!(SetOperatingPoint(1)), msg_ok_return!(SetOperatingPoint)),
                // The test queries for current operating point
                (msg_eq!(GetOperatingPoint), msg_ok_return!(GetOperatingPoint(1))),
                // CpuControlHandler changes operating point to 2
                (msg_eq!(SetOperatingPoint(2)), msg_ok_return!(SetOperatingPoint)),
                // The test queries for current operating point
                (msg_eq!(GetOperatingPoint), msg_ok_return!(GetOperatingPoint(1))),
            ],
        );
        let cpu_ctrl_node = CpuControlHandlerBuilder::new()
            .sustainable_power(power_consumption[0].mul_scalar(1.5))
            .power_gain(power_consumption[0].mul_scalar(0.015))
            .cpu_stats_handler(stats_node)
            .cpu_dev_handler(devhost_node.clone())
            .cpu_ctrl_proxy(fake_cpu_ctrl_driver_with_opps(cpu_params.opps))
            .capacitance(cpu_params.capacitance)
            .logical_cpu_numbers(cpu_params.logical_cpu_numbers)
            .build_and_init()
            .await;

        // Test case 1: Select a thermal load that allows power consumption of the highest opp;
        // expect to be in opp 0
        let available_power = power_consumption[0].mul_scalar(1.01);
        // Thermal load is calculated to ensure that `available_power` is the approximate output of
        // `calculate_available_power`
        let thermal_load = ThermalLoad(
            ((cpu_ctrl_node.sustainable_power.0 - available_power.0) / cpu_ctrl_node.power_gain.0)
                as u32,
        );
        let result = cpu_ctrl_node.handle_message(&Message::UpdateThermalLoad(thermal_load)).await;
        result.unwrap();
        assert_eq!(get_operating_point(devhost_node.clone()).await, 0);

        // Test case 2: Select a thermal load that lowers power consumption to that of opp 1;
        // expect to be in opp 1
        let available_power = power_consumption[1].mul_scalar(1.01);
        // Thermal load is calculated to ensure that `available_power` is the approximate output of
        // `calculate_available_power`
        let thermal_load = ThermalLoad(
            ((cpu_ctrl_node.sustainable_power.0 - available_power.0) / cpu_ctrl_node.power_gain.0)
                as u32,
        );
        let result = cpu_ctrl_node.handle_message(&Message::UpdateThermalLoad(thermal_load)).await;
        result.unwrap();
        assert_eq!(get_operating_point(devhost_node.clone()).await, 1);

        // Test case 3: Select a thermal load that reduce the power consumption limit below the
        // lowest opp; expect to drop to the lowest opp
        let available_power = Watts(0.0);
        // Thermal load is calculated to ensure that `available_power` is the approximate output of
        // `calculate_available_power`
        let thermal_load = ThermalLoad(
            ((cpu_ctrl_node.sustainable_power.0 - available_power.0) / cpu_ctrl_node.power_gain.0)
                as u32,
        );
        let result = cpu_ctrl_node.handle_message(&Message::UpdateThermalLoad(thermal_load)).await;
        result.unwrap();
        assert_eq!(get_operating_point(devhost_node.clone()).await, 1);
    }

    /// Tests that when a minimum CPU clock speed is specified, a opp with a lower CPU frequency
    /// is never selected.
    #[fuchsia::test]
    async fn test_min_cpu_clock_speed() {
        let mut mock_maker = MockNodeMaker::new();

        // Arbitrary CpuControlParams chosen to allow the node to demonstrate opp selection
        let capacitance = Farads(100.0e-12);
        let logical_cpu_numbers = vec![0, 1, 2, 3];
        let opps = vec![
            OperatingPoint { frequency: Hertz(2.0e9), voltage: Volts(3.0) },
            OperatingPoint { frequency: Hertz(1.0e9), voltage: Volts(3.0) },
            OperatingPoint { frequency: Hertz(0.5e9), voltage: Volts(3.0) },
        ];

        // The modeled power consumption at each opp
        let power_consumption: Vec<Watts> = opps
            .iter()
            .map(|opp| {
                get_cpu_power(
                    capacitance,
                    opp.voltage,
                    Hertz(opp.frequency.0 * logical_cpu_numbers.len() as f64),
                )
            })
            .collect();

        let stats_node = mock_maker.make(
            "StatsNode",
            // The CpuControlHandler node queries the current CPU load each time it receives a
            // UpdateThermalLoad message
            vec![
                (msg_eq!(GetCpuLoads), msg_ok_return!(GetCpuLoads(vec![1.0; 4]))),
                (msg_eq!(GetCpuLoads), msg_ok_return!(GetCpuLoads(vec![1.0; 4]))),
            ],
        );
        let devhost_node = mock_maker.make(
            "DevHostNode",
            vec![
                // CpuControlHandler lazy queries operating point during its initialization
                (msg_eq!(GetOperatingPoint), msg_ok_return!(GetOperatingPoint(0))),
                // The test queries for current operating point
                (msg_eq!(GetOperatingPoint), msg_ok_return!(GetOperatingPoint(0))),
                // CpuControlHandler changes operating point to 1
                (msg_eq!(SetOperatingPoint(1)), msg_ok_return!(SetOperatingPoint)),
                // The test queries for current operating point
                (msg_eq!(GetOperatingPoint), msg_ok_return!(GetOperatingPoint(1))),
            ],
        );

        let cpu_ctrl_node = CpuControlHandlerBuilder::new()
            .sustainable_power(power_consumption[0].mul_scalar(1.5))
            .power_gain(power_consumption[0].mul_scalar(0.015))
            .cpu_ctrl_proxy(fake_cpu_ctrl_driver_with_opps(opps))
            .capacitance(capacitance)
            .logical_cpu_numbers(logical_cpu_numbers)
            .cpu_stats_handler(stats_node)
            .cpu_dev_handler(devhost_node.clone())
            .min_cpu_clock_speed(Hertz(1.0e9))
            .build_and_init()
            .await;

        // Test case 1: Select a thermal load that allows power consumption of the highest opp;
        // expect to be in opp 0
        let available_power = power_consumption[0].mul_scalar(1.01);
        // Thermal load is calculated to ensure that `available_power` is the approximate output of
        // `calculate_available_power`
        let thermal_load = ThermalLoad(
            ((cpu_ctrl_node.sustainable_power.0 - available_power.0) / cpu_ctrl_node.power_gain.0)
                as u32,
        );
        let result = cpu_ctrl_node.handle_message(&Message::UpdateThermalLoad(thermal_load)).await;
        result.unwrap();
        assert_eq!(get_operating_point(devhost_node.clone()).await, 0);

        // Test case 2: Select a thermal load that reduces power consumption to below the lowest
        // opp; expect to be in opp 1 (state 2 should be disallowed).
        let available_power = Watts(0.0);
        // Thermal load is calculated to ensure that `available_power` is the approximate output of
        // `calculate_available_power`
        let thermal_load = ThermalLoad(
            ((cpu_ctrl_node.sustainable_power.0 - available_power.0) / cpu_ctrl_node.power_gain.0)
                as u32,
        );
        let result = cpu_ctrl_node.handle_message(&Message::UpdateThermalLoad(thermal_load)).await;
        result.unwrap();
        assert_eq!(get_operating_point(devhost_node.clone()).await, 1);
    }

    /// Tests for the presence and correctness of dynamically-added inspect data
    #[fuchsia::test]
    async fn test_inspect_data() {
        let mut mock_maker = MockNodeMaker::new();
        let devhost_node = mock_maker.make(
            "DevHostNode",
            vec![
                // CpuControlHandler queries operating point during its initialization
                (msg_eq!(GetOperatingPoint), msg_ok_return!(GetOperatingPoint(0))),
            ],
        );

        // Some dummy CpuControlParams to verify the params get published in Inspect
        let opp = OperatingPoint { frequency: Hertz(2.0e9), voltage: Volts(4.5) };
        let capacitance = Farads(100.0e-12);
        let logical_cpu_numbers = vec![0, 1, 2, 3];

        let inspector = inspect::Inspector::default();

        let _node = CpuControlHandlerBuilder::new()
            .cpu_ctrl_proxy(fake_cpu_ctrl_driver_with_opps(vec![opp]))
            .cpu_dev_handler(devhost_node)
            .capacitance(capacitance)
            .logical_cpu_numbers(logical_cpu_numbers)
            .inspect_root(inspector.root())
            .build_and_init()
            .await;

        assert_data_tree!(
            inspector,
            root: {
                "CpuControlHandler (perf_rank: 0)": contains {
                    cpu_control_params: {
                        "capacitance (F)": 100.0e-12,
                        logical_cpu_numbers: "[0, 1, 2, 3]",
                        opp_0: {
                            "voltage (V)": 4.5,
                            "frequency (Hz)": 2.0e9
                        }
                    },
                }
            }
        );
    }

    /// Tests that well-formed configuration JSON does not panic the `new_from_json` function.
    #[fuchsia::test]
    async fn test_new_from_json() {
        let json_data = json::json!({
            "type": "CpuControlHandler",
            "name": "cpu_control",
            "config": {
                "sustainable_power": 0.952,
                "power_gain": 0.0096,
                "total_domain_count": 1,
                "perf_rank": 0,
                "capacitance": 1.2E-10,
                "min_cpu_clock_speed": 1.0e9,
                "logical_cpu_numbers": [0, 1]
            },
            "dependencies": {
                "cpu_stats_handler_node": "cpu_stats",
                "cpu_dev_handler_node": "cpu_dev"
            }
        });

        let mut mock_maker = MockNodeMaker::new();
        let mut nodes: HashMap<String, Rc<dyn Node>> = HashMap::new();
        nodes.insert("cpu_stats".to_string(), mock_maker.make("MockNode", vec![]));
        nodes.insert("cpu_dev".to_string(), mock_maker.make("MockNode", vec![]));
        let _ = CpuControlHandlerBuilder::new_from_json(json_data, &nodes);
    }

    /// Tests that node config files do not contain instances of CpuControlHandler nodes with
    /// overlapping CPU numbers.
    #[fuchsia::test]
    pub fn test_config_files() -> Result<(), anyhow::Error> {
        crate::common_utils::test_each_node_config_file(|config_file| {
            let cpu_control_handlers =
                config_file.iter().filter(|n| n["type"] == "CpuControlHandler");

            let mut set = HashSet::new();
            for node in cpu_control_handlers {
                for cpu in node["config"]["logical_cpu_numbers"].as_array().unwrap() {
                    let cpu_idx = cpu.as_i64().unwrap();
                    if set.contains(&cpu_idx) {
                        return Err(format_err!("CPU {} already specified", cpu_idx));
                    }

                    set.insert(cpu_idx);
                }
            }

            Ok(())
        })
    }
}
