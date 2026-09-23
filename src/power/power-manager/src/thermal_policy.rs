// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::common_utils::get_current_timestamp;
use crate::log_if_err;
use crate::message::{Message, MessageReturn};
use crate::node::Node;
use crate::platform_metrics::PlatformMetric;
use crate::temperature_handler::TemperatureFilter;
use crate::timer::get_periodic_timer_stream;
use crate::types::{Celsius, Nanoseconds, Seconds, ThermalLoad};
use anyhow::{Error, format_err};
use async_trait::async_trait;
use fuchsia_inspect::{self as inspect, Property};
use futures::StreamExt;
use futures::future::{FutureExt, LocalBoxFuture};
use futures::stream::FuturesUnordered;
use log::*;
use serde_derive::Deserialize;
use serde_json as json;
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

/// Node: ThermalPolicy
///
/// Summary: Implements the closed loop thermal control policy for the system
///
/// Handles Messages: N/A
///
/// Sends Messages:
///     - ReadTemperature
///     - SystemShutdown
///     - UpdateThermalLoad
///     - LogPlatformMetric
///     - GetSensorName
///
/// FIDL dependencies: N/A

pub struct ThermalPolicyBuilder<'a> {
    config: ThermalConfig,
    inspect_root: Option<&'a inspect::Node>,
}

impl<'a> ThermalPolicyBuilder<'a> {
    pub fn new(config: ThermalConfig) -> Self {
        Self { config, inspect_root: None }
    }

    pub fn new_from_json(json_data: json::Value, nodes: &HashMap<String, Rc<dyn Node>>) -> Self {
        #[derive(Deserialize)]
        struct ControllerConfig {
            sample_interval: f64,
            filter_time_constant: f64,
            target_temperature: f64,
            e_integral_min: f64,
            e_integral_max: f64,
        }

        #[derive(Deserialize)]
        struct NodeConfig {
            thermal_shutdown_temperature: f64,
            controller_params: ControllerConfig,
        }

        #[derive(Deserialize)]
        struct Dependencies {
            system_power_handler_node: String,
            temperature_handler_node: String,
            thermal_load_notify_nodes: Vec<String>,
            cpu_thermal_load_notify_node: Option<String>,
            platform_metrics_node: String,
        }

        #[derive(Deserialize)]
        struct JsonData {
            config: NodeConfig,
            dependencies: Dependencies,
        }

        let data: JsonData = json::from_value(json_data).unwrap();
        let thermal_config = ThermalConfig {
            temperature_node: nodes[&data.dependencies.temperature_handler_node].clone(),
            sys_pwr_handler: nodes[&data.dependencies.system_power_handler_node].clone(),
            thermal_load_notify_nodes: data
                .dependencies
                .thermal_load_notify_nodes
                .iter()
                .map(|node_name| nodes[node_name].clone())
                .collect(),
            cpu_thermal_load_notify_node: data
                .dependencies
                .cpu_thermal_load_notify_node
                .map(|p| nodes[&p].clone()),
            platform_metrics_node: nodes[&data.dependencies.platform_metrics_node].clone(),
            policy_params: ThermalPolicyParams {
                controller_params: ThermalControllerParams {
                    sample_interval: Seconds(data.config.controller_params.sample_interval),
                    filter_time_constant: Seconds(
                        data.config.controller_params.filter_time_constant,
                    ),
                    target_temperature: Celsius(data.config.controller_params.target_temperature),
                    e_integral_min: data.config.controller_params.e_integral_min,
                    e_integral_max: data.config.controller_params.e_integral_max,
                },
                thermal_shutdown_temperature: Celsius(data.config.thermal_shutdown_temperature),
            },
        };

        Self::new(thermal_config)
    }

    #[cfg(test)]
    fn with_inspect_root(mut self, root: &'a inspect::Node) -> Self {
        self.inspect_root = Some(root);
        self
    }

    pub async fn build<'b>(
        self,
        futures_out: &FuturesUnordered<LocalBoxFuture<'b, ()>>,
    ) -> Result<Rc<ThermalPolicy>, Error> {
        // Create default values
        let inspect_root =
            self.inspect_root.unwrap_or_else(|| inspect::component::inspector().root());

        // Query the TemperatureHandler node for its associated sensor name
        let sensor_name = query_sensor_name(&self.config.temperature_node).await;

        let node = Rc::new(ThermalPolicy {
            sensor_name,
            state: ThermalState {
                temperature_filter: TemperatureFilter::new(
                    self.config.temperature_node.clone(),
                    self.config.policy_params.controller_params.filter_time_constant,
                ),
                prev_timestamp: Cell::new(Nanoseconds(0)),
                max_time_delta: Cell::new(Seconds(0.0)),
                error_integral: Cell::new(0.0),
            },
            inspect: InspectData::new(inspect_root, "ThermalPolicy".to_string(), &self.config),
            config: self.config,
        });

        futures_out.push(node.clone().periodic_thermal_loop());
        Ok(node)
    }
}

/// Queries the provided `temperature_handler` node for their associated sensor name. If any error
/// or unexpected response is encountered then an empty string is returned.
async fn query_sensor_name(temperature_handler: &Rc<dyn Node>) -> String {
    match temperature_handler.handle_message(&Message::GetSensorName).await {
        Ok(MessageReturn::GetSensorName(name)) => name,
        _ => String::new(),
    }
}

pub struct ThermalPolicy {
    /// Sensor name the temperature driver that we're bound to.
    sensor_name: String,

    config: ThermalConfig,
    state: ThermalState,

    /// A struct for managing Component Inspection data.
    inspect: InspectData,
}

/// A struct to store all configurable aspects of the ThermalPolicy node
pub struct ThermalConfig {
    /// The node to provide temperature readings for the thermal control loop. It is expected that
    /// this node responds to the ReadTemperature message.
    pub temperature_node: Rc<dyn Node>,

    /// The node to handle system power state changes (e.g., shutdown). It is expected that this
    /// node responds to the SystemShutdown message.
    pub sys_pwr_handler: Rc<dyn Node>,

    /// The nodes that are interested in receiving updated thermal load values. It is expected that
    /// these nodes respond to the UpdateThermalLoad message.
    pub thermal_load_notify_nodes: Vec<Rc<dyn Node>>,

    /// The node that is interested in receiving updated CPU thermal load values. It is expected
    /// that this node respond to the UpdateCpuThermalLoad message.
    pub cpu_thermal_load_notify_node: Option<Rc<dyn Node>>,

    /// All parameter values relating to the thermal policy itself
    pub policy_params: ThermalPolicyParams,

    /// Node that we'll notify with relevant platform metrics.
    pub platform_metrics_node: Rc<dyn Node>,
}

/// A struct to store all configurable aspects of the thermal policy itself
pub struct ThermalPolicyParams {
    /// The thermal control loop parameters
    pub controller_params: ThermalControllerParams,

    /// If temperature reaches or exceeds this value, the policy will command a system shutdown
    pub thermal_shutdown_temperature: Celsius,
}

/// A struct to store the tunable thermal control loop parameters
#[derive(Clone, Debug)]
pub struct ThermalControllerParams {
    /// The interval at which to run the thermal control loop
    pub sample_interval: Seconds,

    /// Time constant for the low-pass filter used for smoothing the temperature input signal
    pub filter_time_constant: Seconds,

    /// Target temperature for the integral control calculation
    pub target_temperature: Celsius,

    /// Minimum integral error [degC * s] for the integral control calculation
    pub e_integral_min: f64,

    /// Maximum integral error [degC * s] for the integral control calculation
    pub e_integral_max: f64,
}

/// State information that is used for calculations across controller iterations
struct ThermalState {
    /// Provides filtered temperature values according to the configured filter constant.
    temperature_filter: TemperatureFilter,

    /// The time of the previous controller iteration
    prev_timestamp: Cell<Nanoseconds>,

    /// The largest observed time between controller iterations (may be used to detect hangs)
    max_time_delta: Cell<Seconds>,

    /// The integral error [degC * s] that is accumulated across controller iterations
    error_integral: Cell<f64>,
}

impl ThermalPolicy {
    /// Creates a Future driven by a timer firing at the interval specified by
    /// ThermalControllerParams.sample_interval. At each fire, `iterate_thermal_control` is called
    /// and any resulting errors are logged.
    fn periodic_thermal_loop<'a>(self: Rc<Self>) -> LocalBoxFuture<'a, ()> {
        let mut periodic_timer = get_periodic_timer_stream(
            self.config.policy_params.controller_params.sample_interval.into(),
        );

        async move {
            while let Some(()) = periodic_timer.next().await {
                fuchsia_trace::instant!(
                    c"power_manager",
                    c"ThermalPolicy::periodic_timer_fired",
                    fuchsia_trace::Scope::Thread
                );
                let result = self.iterate_thermal_control().await;
                log_if_err!(result, "Error while running thermal control iteration");
                fuchsia_trace::instant!(
                    c"power_manager",
                    c"ThermalPolicy::iterate_thermal_control_result",
                    fuchsia_trace::Scope::Thread,
                    "result" => format!("{:?}", result).as_str()
                );
            }
        }
        .boxed_local()
    }

    /// This is the main body of the closed loop thermal control logic. The function is called
    /// periodically by the timer started in `start_periodic_thermal_loop`. For each iteration, the
    /// following steps will be taken:
    ///     1. Read the current temperature from the temperature driver specified in ThermalConfig
    ///     2. Filter the raw temperature value using a low-pass filter
    ///     3. Use the new filtered temperature to calculate the integral error of temperature
    ///        relative to the configured target temperature
    ///     4. Use the integral error to derive thermal load and available power values
    ///     5. Update the relevant nodes with the new thermal load and available power information
    async fn iterate_thermal_control(&self) -> Result<(), Error> {
        fuchsia_trace::duration!("power_manager", "ThermalPolicy::iterate_thermal_control");

        let timestamp = get_current_timestamp();
        let time_delta = self.get_time_delta(timestamp);

        let temperature = self.state.temperature_filter.get_temperature(timestamp).await?;
        let error_integral = self.get_temperature_error(temperature.filtered, time_delta);
        let thermal_load = Self::calculate_thermal_load(
            error_integral,
            self.config.policy_params.controller_params.e_integral_min,
            self.config.policy_params.controller_params.e_integral_max,
        );

        self.log_thermal_iteration_metrics(
            timestamp,
            time_delta,
            temperature.raw,
            temperature.filtered,
            error_integral,
            thermal_load,
        );

        // If the new temperature is above the critical threshold then shut down the system
        let result = self.check_critical_temperature(temperature.raw).await;
        log_if_err!(result, "Error checking critical temperature");
        fuchsia_trace::instant!(
            c"power_manager",
            c"ThermalPolicy::check_critical_temperature_result",
            fuchsia_trace::Scope::Thread,
            "result" => format!("{:?}", result).as_str()
        );

        // Determine the new thermal load and update `thermal_load_notify_nodes`
        let result = self.process_thermal_load(thermal_load).await;
        log_if_err!(result, "Error updating thermal load");
        fuchsia_trace::instant!(
            c"power_manager",
            c"ThermalPolicy::process_thermal_load_result",
            fuchsia_trace::Scope::Thread,
            "result" => format!("{:?}", result).as_str()
        );

        Ok(())
    }

    /// Gets the time delta from the previous call to this function using the provided timestamp.
    /// Logs the largest delta into Inspect.
    fn get_time_delta(&self, timestamp: Nanoseconds) -> Seconds {
        let time_delta = (timestamp - self.state.prev_timestamp.get()).into();
        if time_delta > self.state.max_time_delta.get().into() {
            self.state.max_time_delta.set(time_delta);
            self.inspect.max_time_delta.set(time_delta.0);
        }
        self.state.prev_timestamp.set(timestamp);
        time_delta
    }

    /// Calculates integral error of temperature using the provided input temperature and time
    /// delta. Stores the new integral error and logs it to Inspect.
    fn get_temperature_error(&self, temperature: Celsius, time_delta: Seconds) -> f64 {
        let controller_params = &self.config.policy_params.controller_params;
        let error_proportional = controller_params.target_temperature.0 - temperature.0;
        let error_integral = num_traits::clamp(
            self.state.error_integral.get() + error_proportional * time_delta.0,
            controller_params.e_integral_min,
            controller_params.e_integral_max,
        );
        self.state.error_integral.set(error_integral);
        self.inspect.error_integral.set(error_integral);
        error_integral
    }

    /// Logs various state data that is updated on each iteration of the thermal policy.
    fn log_thermal_iteration_metrics(
        &self,
        timestamp: Nanoseconds,
        time_delta: Seconds,
        raw_temperature: Celsius,
        filtered_temperature: Celsius,
        temperature_error_integral: f64,
        thermal_load: ThermalLoad,
    ) {
        self.inspect.timestamp.set(timestamp.0);
        self.inspect.time_delta.set(time_delta.0);
        self.inspect.temperature_raw.set(raw_temperature.0);
        self.inspect.temperature_filtered.set(filtered_temperature.0);
        self.inspect.thermal_load.set(thermal_load.0.into());
        fuchsia_trace::instant!(
            c"power_manager",
            c"ThermalPolicy::thermal_control_iteration_data",
            fuchsia_trace::Scope::Thread,
            "timestamp" => timestamp.0
        );
        fuchsia_trace::counter!(
            c"power_manager",
            c"ThermalPolicy raw_temperature",
            0,
            "raw_temperature" => raw_temperature.0
        );
        fuchsia_trace::counter!(
            c"power_manager",
            c"ThermalPolicy filtered_temperature",
            0,
            "filtered_temperature" => filtered_temperature.0
        );
        fuchsia_trace::counter!(
            c"power_manager",
            c"ThermalPolicy error_integral", 0,
            "error_integral" => temperature_error_integral
        );
        fuchsia_trace::counter!(
            c"power_manager",
            c"ThermalPolicy thermal_load",
            0,
            "thermal_load" => thermal_load.0
        );
    }

    /// Compares the supplied temperature with the thermal config thermal shutdown temperature. If
    /// we've reached or exceeded the shutdown temperature, message the system power handler node
    /// to initiate a system shutdown.
    async fn check_critical_temperature(&self, temperature: Celsius) -> Result<(), Error> {
        fuchsia_trace::duration!(
            c"power_manager",
            c"ThermalPolicy::check_critical_temperature",
            "temperature" => temperature.0
        );

        // Temperature has exceeded the thermal shutdown temperature
        if temperature.0 >= self.config.policy_params.thermal_shutdown_temperature.0 {
            info!(
                "Thermal shutdown triggered: temperature {:.1}°C reached shutdown threshold {:.1}°C",
                temperature.0, self.config.policy_params.thermal_shutdown_temperature.0
            );

            fuchsia_trace::instant!(
                c"power_manager",
                c"ThermalPolicy::thermal_shutdown_reached",
                fuchsia_trace::Scope::Thread,
                "temperature" => temperature.0,
                "shutdown_temperature" => self.config.policy_params.thermal_shutdown_temperature.0
            );

            self.log_platform_metric(PlatformMetric::ThrottlingResultShutdown).await;

            self.send_message(&self.config.sys_pwr_handler, &Message::HighTemperatureShutdown)
                .await
                .map_err(|e| format_err!("Failed to shut down the system: {}", e))?;
        }

        Ok(())
    }

    /// Process a new thermal load value. If there is a change from the cached thermal_load, then
    /// the new value is sent out to `thermal_load_notify_nodes`.
    async fn process_thermal_load(&self, new_load: ThermalLoad) -> Result<(), Error> {
        fuchsia_trace::duration!(
            c"power_manager",
            c"ThermalPolicy::process_thermal_load",
            "new_load" => new_load.0
        );

        self.send_message_to_many(
            &self.config.thermal_load_notify_nodes,
            &Message::UpdateThermalLoad(new_load, self.sensor_name.to_string()),
        )
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;

        if let Some(cpu_thermal_load_notify_node) = &self.config.cpu_thermal_load_notify_node {
            self.send_message(
                &cpu_thermal_load_notify_node,
                &Message::UpdateCpuThermalLoad(new_load),
            )
            .await
            .map_err(|e| format_err!("Failed to send cpu thermal load: {}", e))?;
        }

        Ok(())
    }

    async fn log_platform_metric(&self, metric: PlatformMetric) {
        let msg = Message::LogPlatformMetric(metric);
        log_if_err!(
            self.send_message(&self.config.platform_metrics_node, &msg).await,
            format!("Failed to log platform metric {:?}", msg)
        );
    }

    /// Calculates the thermal load which is a value in the range [0 - MAX_THERMAL_LOAD] defined as
    /// ((error_integral - range_start) / (range_end - range_start) * MAX_THERMAL_LOAD), where the
    /// range is defined by the maximum and minimum integral error according to the controller
    /// parameters.
    fn calculate_thermal_load(
        error_integral: f64,
        error_integral_min: f64,
        error_integral_max: f64,
    ) -> ThermalLoad {
        debug_assert!(
            error_integral >= error_integral_min,
            "error_integral ({}) less than error_integral_min ({})",
            error_integral,
            error_integral_min
        );
        debug_assert!(
            error_integral <= error_integral_max,
            "error_integral ({}) greater than error_integral_max ({})",
            error_integral,
            error_integral_max
        );

        if error_integral < error_integral_min {
            error!(
                "error_integral {} less than error_integral_min {}",
                error_integral, error_integral_min
            );
            ThermalLoad(100)
        } else if error_integral > error_integral_max {
            error!(
                "error_integral {} greater than error_integral_max {}",
                error_integral, error_integral_max
            );
            ThermalLoad(0)
        } else {
            ThermalLoad(
                ((error_integral - error_integral_max) / (error_integral_min - error_integral_max)
                    * 100.0) as u32,
            )
        }
    }
}

#[async_trait(?Send)]
impl Node for ThermalPolicy {
    fn name(&self) -> String {
        "ThermalPolicy".to_string()
    }
}

struct InspectData {
    // Nodes
    root_node: inspect::Node,

    // Properties
    timestamp: inspect::IntProperty,
    time_delta: inspect::DoubleProperty,
    temperature_raw: inspect::DoubleProperty,
    temperature_filtered: inspect::DoubleProperty,
    error_integral: inspect::DoubleProperty,
    thermal_load: inspect::UintProperty,
    max_time_delta: inspect::DoubleProperty,
}

impl InspectData {
    fn new(parent: &inspect::Node, name: String, config: &ThermalConfig) -> Self {
        // Create a local root node and properties
        let root_node = parent.create_child(name);
        let state_node = root_node.create_child("state");
        let stats_node = root_node.create_child("stats");
        let timestamp = state_node.create_int("timestamp (ns)", 0);
        let time_delta = state_node.create_double("time_delta (s)", 0.0);
        let temperature_raw = state_node.create_double("temperature_raw (C)", 0.0);
        let temperature_filtered = state_node.create_double("temperature_filtered (C)", 0.0);
        let error_integral = state_node.create_double("error_integral", 0.0);
        let thermal_load = state_node.create_uint("thermal_load", 0);
        let max_time_delta = stats_node.create_double("max_time_delta (s)", 0.0);

        // Pass ownership of the new nodes to the root node, otherwise they'll be dropped
        root_node.record(state_node);
        root_node.record(stats_node);

        let inspect_data = InspectData {
            root_node,
            timestamp,
            time_delta,
            max_time_delta,
            temperature_raw,
            temperature_filtered,
            error_integral,
            thermal_load,
        };
        inspect_data.set_thermal_config(config);
        inspect_data
    }

    fn set_thermal_config(&self, config: &ThermalConfig) {
        let policy_params_node = self.root_node.create_child("policy_params");
        let ctrl_params_node = policy_params_node.create_child("controller_params");
        let params = &config.policy_params.controller_params;
        ctrl_params_node.record_double("sample_interval (s)", params.sample_interval.0);
        ctrl_params_node.record_double("filter_time_constant (s)", params.filter_time_constant.0);
        ctrl_params_node.record_double("target_temperature (C)", params.target_temperature.0);
        ctrl_params_node.record_double("e_integral_min", params.e_integral_min);
        ctrl_params_node.record_double("e_integral_max", params.e_integral_max);
        policy_params_node.record(ctrl_params_node);

        self.root_node.record(policy_params_node);
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::test::mock_node::{MessageMatcher, MockNodeMaker, create_dummy_node};
    use crate::{msg_eq, msg_ok_return};
    use diagnostics_assertions::assert_data_tree;
    use fuchsia_async as fasync;

    fn default_policy_params() -> ThermalPolicyParams {
        ThermalPolicyParams {
            controller_params: ThermalControllerParams {
                sample_interval: Seconds(1.0),
                filter_time_constant: Seconds(10.0),
                target_temperature: Celsius(85.0),
                e_integral_min: -20.0,
                e_integral_max: 0.0,
            },
            thermal_shutdown_temperature: Celsius(95.0),
        }
    }

    /// Tests the calculate_thermal_load function for correctness.
    #[test] // Use instead of #[fuchsia::test] to suppress ERROR logs that cause test failure
    fn test_calculate_thermal_load() {
        // These tests using invalid error integral values will panic on debug builds and the
        // `test_calculate_thermal_load_error_integral_*` tests will verify that. For now (non-debug
        // build), just test that the invalid values are clamped to a valid ThermalLoad value.
        if cfg!(not(debug_assertions)) {
            // An invalid error_integral greater than e_integral_max should clamp at ThermalLoad(0)
            assert_eq!(ThermalPolicy::calculate_thermal_load(5.0, -20.0, 0.0), ThermalLoad(0));

            // An invalid error_integral less than e_integral_min should clamp at ThermalLoad(100)
            assert_eq!(ThermalPolicy::calculate_thermal_load(-25.0, -20.0, 0.0), ThermalLoad(100));
        }

        // Test some error_integral values ranging from e_integral_max to e_integral_min
        assert_eq!(ThermalPolicy::calculate_thermal_load(0.0, -20.0, 0.0), ThermalLoad(0));
        assert_eq!(ThermalPolicy::calculate_thermal_load(-10.0, -20.0, 0.0), ThermalLoad(50));
        assert_eq!(ThermalPolicy::calculate_thermal_load(-20.0, -20.0, 0.0), ThermalLoad(100));
    }

    /// Tests that an invalid low error integral value will cause a panic on debug builds.
    #[test] // Use instead of #[fuchsia::test] to suppress ERROR logs that cause test failure
    #[should_panic = "error_integral (-25) less than error_integral_min (-20)"]
    #[cfg(debug_assertions)]
    fn test_calculate_thermal_load_error_integral_low_panic() {
        assert_eq!(ThermalPolicy::calculate_thermal_load(-25.0, -20.0, 0.0), ThermalLoad(100));
    }

    /// Tests that an invalid high error integral value will cause a panic on debug builds.
    #[test] // Use instead of #[fuchsia::test] to suppress ERROR logs that cause test failure
    #[should_panic = "error_integral (5) greater than error_integral_max (0)"]
    #[cfg(debug_assertions)]
    fn test_calculate_thermal_load_error_integral_high_panic() {
        assert_eq!(ThermalPolicy::calculate_thermal_load(5.0, -20.0, 0.0), ThermalLoad(0));
    }

    /// Tests that the `get_time_delta` function correctly calculates time delta while updating the
    /// `max_time_delta` state variable.
    #[fuchsia::test]
    async fn test_get_time_delta() {
        let thermal_config = ThermalConfig {
            temperature_node: create_dummy_node(),
            sys_pwr_handler: create_dummy_node(),
            thermal_load_notify_nodes: vec![create_dummy_node()],
            cpu_thermal_load_notify_node: None,
            policy_params: default_policy_params(),
            platform_metrics_node: create_dummy_node(),
        };

        let node_futures = FuturesUnordered::new();
        let node = ThermalPolicyBuilder::new(thermal_config).build(&node_futures).await.unwrap();

        assert_eq!(node.get_time_delta(Seconds(1.5).into()), Seconds(1.5));
        assert_eq!(node.get_time_delta(Seconds(2.0).into()), Seconds(0.5));
        assert_eq!(node.state.max_time_delta.get(), Seconds(1.5));
    }

    /// Tests that the `get_temperature_error` function correctly calculates integral error.
    #[fuchsia::test]
    async fn test_get_temperature_error() {
        let mut policy_params = default_policy_params();
        policy_params.controller_params.target_temperature = Celsius(80.0);
        policy_params.controller_params.e_integral_min = -20.0;
        policy_params.controller_params.e_integral_max = 0.0;

        let thermal_config = ThermalConfig {
            temperature_node: create_dummy_node(),
            sys_pwr_handler: create_dummy_node(),
            thermal_load_notify_nodes: vec![create_dummy_node()],
            cpu_thermal_load_notify_node: None,
            policy_params,
            platform_metrics_node: create_dummy_node(),
        };
        let node_futures = FuturesUnordered::new();
        let node = ThermalPolicyBuilder::new(thermal_config).build(&node_futures).await.unwrap();

        assert_eq!(node.get_temperature_error(Celsius(40.0), Seconds(1.0)), 0.0);
        assert_eq!(node.get_temperature_error(Celsius(90.0), Seconds(1.0)), -10.0);
        assert_eq!(node.get_temperature_error(Celsius(90.0), Seconds(1.0)), -20.0);
    }

    /// Tests for the presence and correctness of dynamically-added inspect data
    #[fuchsia::test]
    async fn test_inspect_data() {
        let mut mock_maker = MockNodeMaker::new();

        let policy_params = default_policy_params();
        let thermal_config = ThermalConfig {
            temperature_node: mock_maker.make(
                "TemperatureNode",
                vec![(msg_eq!(GetSensorName), msg_ok_return!(GetSensorName(String::new())))],
            ),
            sys_pwr_handler: mock_maker.make("SysPwrNode", vec![]),
            thermal_load_notify_nodes: vec![mock_maker.make("ThermalLoadNotify", vec![])],
            cpu_thermal_load_notify_node: None,
            policy_params: default_policy_params(),
            platform_metrics_node: create_dummy_node(),
        };
        let inspector = inspect::Inspector::default();
        let node_futures = FuturesUnordered::new();
        let _node = ThermalPolicyBuilder::new(thermal_config)
            .with_inspect_root(inspector.root())
            .build(&node_futures)
            .await
            .unwrap();

        assert_data_tree!(
            inspector,
            root: {
                ThermalPolicy: {
                    state: contains {},
                    stats: contains {},
                    policy_params: {
                        controller_params: {
                            "sample_interval (s)":
                                policy_params.controller_params.sample_interval.0,
                            "filter_time_constant (s)":
                                policy_params.controller_params.filter_time_constant.0,
                            "target_temperature (C)":
                                policy_params.controller_params.target_temperature.0,
                            "e_integral_min": policy_params.controller_params.e_integral_min,
                            "e_integral_max": policy_params.controller_params.e_integral_max,
                        }
                    }
                }
            }
        );
    }

    /// Tests that well-formed configuration JSON does not panic the `new_from_json` function.
    #[fuchsia::test]
    fn test_new_from_json() {
        let json_data = json::json!({
            "type": "ThermalPolicy",
            "name": "thermal_policy",
            "config": {
                "thermal_shutdown_temperature": 95.0,
                "controller_params": {
                    "sample_interval": 1.0,
                    "filter_time_constant": 5.0,
                    "target_temperature": 80.0,
                    "e_integral_min": -20.0,
                    "e_integral_max": 0.0,
                }
            },
            "dependencies": {
                "system_power_handler_node": "sys_power",
                "temperature_handler_node": "temperature",
                "thermal_load_notify_nodes": ["thermal_load_notify", "cpu_control"],
                "platform_metrics_node": "metrics"
              },
        });

        let mut nodes: HashMap<String, Rc<dyn Node>> = HashMap::new();
        nodes.insert("temperature".to_string(), create_dummy_node());
        nodes.insert("sys_power".to_string(), create_dummy_node());
        nodes.insert("thermal_load_notify".to_string(), create_dummy_node());
        nodes.insert("cpu_control".to_string(), create_dummy_node());
        nodes.insert("metrics".to_string(), create_dummy_node());
        let _ = ThermalPolicyBuilder::new_from_json(json_data, &nodes);
    }

    #[fuchsia::test]
    fn test_new_from_json_with_cpu_thermal_load_notify_node() {
        let json_data = json::json!({
            "type": "ThermalPolicy",
            "name": "thermal_policy",
            "config": {
                "thermal_shutdown_temperature": 95.0,
                "controller_params": {
                    "sample_interval": 1.0,
                    "filter_time_constant": 5.0,
                    "target_temperature": 80.0,
                    "e_integral_min": -20.0,
                    "e_integral_max": 0.0,
                }
            },
            "dependencies": {
                "system_power_handler_node": "sys_power",
                "temperature_handler_node": "temperature",
                "thermal_load_notify_nodes": ["thermal_load_notify", "cpu_control"],
                "cpu_thermal_load_notify_node": "cpu_thermal_load_notify",
                "platform_metrics_node": "metrics"
              },
        });

        let mut nodes: HashMap<String, Rc<dyn Node>> = HashMap::new();
        nodes.insert("temperature".to_string(), create_dummy_node());
        nodes.insert("sys_power".to_string(), create_dummy_node());
        nodes.insert("thermal_load_notify".to_string(), create_dummy_node());
        nodes.insert("cpu_thermal_load_notify".to_string(), create_dummy_node());
        nodes.insert("cpu_control".to_string(), create_dummy_node());
        nodes.insert("metrics".to_string(), create_dummy_node());
        let _ = ThermalPolicyBuilder::new_from_json(json_data, &nodes);
    }

    /// Tests that the ThermalPolicy correctly updates the PlatformMetrics node as its thermal state
    /// cycles between the various states.
    #[fuchsia::test]
    fn test_platform_metrics() {
        let mut mock_maker = MockNodeMaker::new();

        // Set custom thermal policy parameters to have easier control over throttling state changes
        let mut policy_params = default_policy_params();
        policy_params.controller_params.target_temperature = Celsius(50.0);
        policy_params.controller_params.e_integral_max = 0.0;
        policy_params.controller_params.e_integral_min = -20.0;
        policy_params.thermal_shutdown_temperature = Celsius(80.0);
        policy_params.controller_params.filter_time_constant = Seconds(0.0);

        // The executor's fake time must be set before node creation to ensure the periodic timer's
        // deadline is properly initialized.
        let mut executor = fasync::TestExecutor::new_with_fake_time();
        executor.set_fake_time(Seconds(0.0).into());

        let mock_metrics = mock_maker.make("MockPlatformMetrics", vec![]);
        let mock_temperature = mock_maker.make(
            "MockTemperature",
            vec![(msg_eq!(GetSensorName), msg_ok_return!(GetSensorName("sensor1".to_string())))],
        );

        let thermal_load_notify_node = mock_maker.make("ThermalLoadConsumer", vec![]);
        let sys_pwr_handler = mock_maker.make("SysPowerHandler", vec![]);

        let node_futures = FuturesUnordered::new();
        let node_builder = ThermalPolicyBuilder::new(ThermalConfig {
            temperature_node: mock_temperature.clone(),
            sys_pwr_handler: sys_pwr_handler.clone(),
            thermal_load_notify_nodes: vec![thermal_load_notify_node.clone()],
            cpu_thermal_load_notify_node: None,
            platform_metrics_node: mock_metrics.clone(),
            policy_params,
        })
        .build(&node_futures);

        let _node = match executor.run_until_stalled(&mut node_builder.boxed_local()) {
            futures::task::Poll::Ready(Ok(node)) => node,
            _ => panic!("Failed to create node"),
        };

        // Wrap the executor, node, and futures in a simple struct that drives time steps. This
        // enables the control flow:
        // 1. Set metric expectations (add expected message to mock node(s)).
        // 2. Call TimeStepper::iterate_policy to wake the periodic timer and iterate the
        //    ThermalPolicy.
        // 3. Verify metric expectations.
        struct TimeStepper<'a> {
            executor: fasync::TestExecutor,
            node_futures: FuturesUnordered<LocalBoxFuture<'a, ()>>,
        }

        impl<'a> TimeStepper<'a> {
            fn iterate_policy(&mut self) {
                assert_eq!(
                    futures::task::Poll::Pending,
                    self.executor.run_until_stalled(&mut self.node_futures.next())
                );

                let wakeup_time = self.executor.wake_next_timer().unwrap();
                self.executor.set_fake_time(wakeup_time);

                assert_eq!(
                    futures::task::Poll::Pending,
                    self.executor.run_until_stalled(&mut self.node_futures.next())
                );
            }
        }

        let mut stepper = TimeStepper { executor, node_futures };

        // Cause the thermal policy to send `ThrottlingResultShutdown`.
        mock_temperature.add_msg_response_pair((
            msg_eq!(ReadTemperature),
            msg_ok_return!(ReadTemperature(Celsius(80.0))),
        ));
        mock_metrics.add_msg_response_pair((
            msg_eq!(LogPlatformMetric(PlatformMetric::ThrottlingResultShutdown)),
            msg_ok_return!(LogPlatformMetric),
        ));
        sys_pwr_handler.add_msg_response_pair((
            msg_eq!(HighTemperatureShutdown),
            msg_ok_return!(SystemShutdown),
        ));
        thermal_load_notify_node.add_msg_response_pair((
            msg_eq!(UpdateThermalLoad(ThermalLoad(100), "sensor1".to_string())),
            msg_ok_return!(UpdateThermalLoad),
        ));
        stepper.iterate_policy();

        // On a real system, the shutdown would have occured. But since the test continues executing
        // even after the shutdown request is sent, we can perform a few more rounds of testing.
        //
        // Temperature is reduced, no shutdown message is sent.
        mock_temperature.add_msg_response_pair((
            msg_eq!(ReadTemperature),
            msg_ok_return!(ReadTemperature(Celsius(50.0))),
        ));
        // Even though the temperature is at the nominal target, the integral error is still
        // stuck at its most negative value. As a result, the thermal load is still 100.
        // (The calculation is extremely artificial in the context of this test; don't try too
        // hard to rationalize it.)
        thermal_load_notify_node.add_msg_response_pair((
            msg_eq!(UpdateThermalLoad(ThermalLoad(100), "sensor1".to_string())),
            msg_ok_return!(UpdateThermalLoad),
        ));
        stepper.iterate_policy();

        // Cause the thermal policy to send `ThrottlingResultShutdown`.
        mock_temperature.add_msg_response_pair((
            msg_eq!(ReadTemperature),
            msg_ok_return!(ReadTemperature(Celsius(90.0))),
        ));
        mock_metrics.add_msg_response_pair((
            msg_eq!(LogPlatformMetric(PlatformMetric::ThrottlingResultShutdown)),
            msg_ok_return!(LogPlatformMetric),
        ));
        sys_pwr_handler.add_msg_response_pair((
            msg_eq!(HighTemperatureShutdown),
            msg_ok_return!(SystemShutdown),
        ));
        thermal_load_notify_node.add_msg_response_pair((
            msg_eq!(UpdateThermalLoad(ThermalLoad(100), "sensor1".to_string())),
            msg_ok_return!(UpdateThermalLoad),
        ));
        stepper.iterate_policy();
    }

    /// Tests that the ThermalPolicy node populates the correct temperature sensor name in its
    /// UpdateThermalLoad messages.
    #[fuchsia::test]
    async fn test_thermal_load_sensor_name() {
        let mut mock_maker = MockNodeMaker::new();

        // Set up the ThermalPolicy node
        let thermal_config = ThermalConfig {
            temperature_node: mock_maker.make(
                "TemperatureNode",
                vec![(
                    msg_eq!(GetSensorName),
                    msg_ok_return!(GetSensorName("Sensor1".to_string())),
                )],
            ),
            sys_pwr_handler: create_dummy_node(),
            thermal_load_notify_nodes: vec![mock_maker.make(
                "ThermalLoadNotify",
                vec![(
                    msg_eq!(UpdateThermalLoad(ThermalLoad(20), "Sensor1".to_string())),
                    msg_ok_return!(UpdateThermalLoad),
                )],
            )],
            cpu_thermal_load_notify_node: None,
            policy_params: default_policy_params(),
            platform_metrics_node: create_dummy_node(),
        };

        let node = ThermalPolicyBuilder::new(thermal_config)
            .build(&FuturesUnordered::new())
            .await
            .unwrap();

        // When `process_thermal_load` runs, the new ThermalLoad value will be sent to the
        // ThermalLoadNotify node. The mock will verify the correct sensor name is found in the
        // UpdateThermalLoad message.
        node.process_thermal_load(ThermalLoad(20)).await.unwrap();
    }

    /// Tests that each of the configured `thermal_load_notify_nodes` nodes receive an
    /// UpdateThermalLoad message as expected.
    #[fuchsia::test]
    async fn test_multiple_thermal_load_notify_nodes() {
        let mut mock_maker = MockNodeMaker::new();

        let mock_notify1 = mock_maker.make("ThermalLoadNotify1", vec![]);
        let mock_notify2 = mock_maker.make("ThermalLoadNotify2", vec![]);

        // Set up the ThermalPolicy node
        let thermal_config = ThermalConfig {
            temperature_node: mock_maker.make(
                "TemperatureNode",
                vec![(
                    msg_eq!(GetSensorName),
                    msg_ok_return!(GetSensorName("Sensor1".to_string())),
                )],
            ),
            sys_pwr_handler: create_dummy_node(),
            thermal_load_notify_nodes: vec![mock_notify1.clone(), mock_notify2.clone()],
            cpu_thermal_load_notify_node: None,
            policy_params: default_policy_params(),
            platform_metrics_node: create_dummy_node(),
        };

        let node = ThermalPolicyBuilder::new(thermal_config)
            .build(&FuturesUnordered::new())
            .await
            .unwrap();

        // When `process_thermal_load` runs, the mocks will verify they each receive the
        // UpdateThermalLoad message
        mock_notify1.add_msg_response_pair((
            msg_eq!(UpdateThermalLoad(ThermalLoad(20), "Sensor1".to_string())),
            msg_ok_return!(UpdateThermalLoad),
        ));
        mock_notify2.add_msg_response_pair((
            msg_eq!(UpdateThermalLoad(ThermalLoad(20), "Sensor1".to_string())),
            msg_ok_return!(UpdateThermalLoad),
        ));
        node.process_thermal_load(ThermalLoad(20)).await.unwrap();

        // Even if thermal load is unchanged, the nodes should still be updated
        mock_notify1.add_msg_response_pair((
            msg_eq!(UpdateThermalLoad(ThermalLoad(20), "Sensor1".to_string())),
            msg_ok_return!(UpdateThermalLoad),
        ));
        mock_notify2.add_msg_response_pair((
            msg_eq!(UpdateThermalLoad(ThermalLoad(20), "Sensor1".to_string())),
            msg_ok_return!(UpdateThermalLoad),
        ));
        node.process_thermal_load(ThermalLoad(20)).await.unwrap();
    }

    /// Tests that each of the configured `cpu_thermal_load_notify_node` node receive an
    /// UpdateCpuThermalLoad message as expected.
    #[fuchsia::test]
    async fn test_cpu_thermal_load_notify_node() {
        let mut mock_maker = MockNodeMaker::new();

        let mock_notify = mock_maker.make("ThermalLoad", vec![]);
        let mock_cpu_notify = mock_maker.make("CpuThermalLoadNotify", vec![]);

        // Set up the ThermalPolicy node
        let thermal_config = ThermalConfig {
            temperature_node: mock_maker.make(
                "TemperatureNode",
                vec![(
                    msg_eq!(GetSensorName),
                    msg_ok_return!(GetSensorName("Sensor1".to_string())),
                )],
            ),
            sys_pwr_handler: create_dummy_node(),
            thermal_load_notify_nodes: vec![mock_notify.clone()],
            cpu_thermal_load_notify_node: Some(mock_cpu_notify.clone()),
            policy_params: default_policy_params(),
            platform_metrics_node: create_dummy_node(),
        };

        let node = ThermalPolicyBuilder::new(thermal_config)
            .build(&FuturesUnordered::new())
            .await
            .unwrap();

        // When `process_thermal_load` runs, the mocks will verify they each receive the
        // UpdateThermalLoad or UpdateCpuThermalLoad message
        mock_notify.add_msg_response_pair((
            msg_eq!(UpdateThermalLoad(ThermalLoad(20), "Sensor1".to_string())),
            msg_ok_return!(UpdateThermalLoad),
        ));
        mock_cpu_notify.add_msg_response_pair((
            msg_eq!(UpdateCpuThermalLoad(ThermalLoad(20))),
            msg_ok_return!(UpdateCpuThermalLoad),
        ));
        node.process_thermal_load(ThermalLoad(20)).await.unwrap();

        // Even if thermal load is unchanged, the nodes should still be updated
        mock_notify.add_msg_response_pair((
            msg_eq!(UpdateThermalLoad(ThermalLoad(20), "Sensor1".to_string())),
            msg_ok_return!(UpdateThermalLoad),
        ));
        mock_cpu_notify.add_msg_response_pair((
            msg_eq!(UpdateCpuThermalLoad(ThermalLoad(20))),
            msg_ok_return!(UpdateCpuThermalLoad),
        ));
        node.process_thermal_load(ThermalLoad(20)).await.unwrap();
    }
}
