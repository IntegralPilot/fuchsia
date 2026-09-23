// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[path = "../../../../common/lib/types.rs"]
mod types;

use log::info;
use named_timer::DeadlineId;
use power_manager_integration_test_lib::{TestEnv, TestEnvBuilder};
use test_util::assert_near;
use types::{Celsius, Farads, Hertz, Nanoseconds, OperatingPoint, Seconds, Volts, Watts};
use {
    fidl_fuchsia_hardware_cpu_ctrl as fcpu_ctrl, fidl_fuchsia_kernel as fkernel,
    fidl_fuchsia_powermanager_driver_temperaturecontrol as ftemperaturecontrol,
    fidl_fuchsia_sys2 as fsys2, fidl_fuchsia_testing as ftesting, fuchsia_async as fasync,
    serde_json as json,
};

const DEADLINE_ID: DeadlineId<'static> = DeadlineId::new("power-manager", "thermal-policy-timer");

/// Initialization parameters for a new Simulator.
struct SimulatorParams {
    /// Parameters for the underlying thermal model.
    thermal_model_params: ThermalModelParams,
    /// Parameters for the simulated CPU.
    cpu_params: SimulatedCpuParams,
    /// Schedules simulated CPU operations.
    op_scheduler: OperationScheduler,
    /// Initial temperature of the CPU.
    initial_cpu_temperature: Celsius,
    /// Initial temperature of the heat sink.
    initial_heat_sink_temperature: Celsius,
    /// Temperature of the environment (constant).
    environment_temperature: Celsius,
}

#[derive(Clone, Debug)]
struct SimulatedCpuParams {
    logical_cpu_numbers: Vec<u32>,
    opps: Vec<OperatingPoint>,
    capacitance: Farads,
}

/// Parameters for a linear thermal model including a CPU, heat sink, and environment.
/// For simplicity, we assume heat flow directly between the CPU and environment is negligible.
#[derive(Clone, Debug)]
struct ThermalModelParams {
    /// Thermal energy transfer rate [W/deg C] between CPU and heat sink.
    cpu_to_heat_sink_thermal_rate: f64,
    /// Thermal energy transfer rate [W/deg C] between heat sink and environment.
    heat_sink_to_env_thermal_rate: f64,
    /// Thermal capacity [J/deg C] of the CPU.
    cpu_thermal_capacity: f64,
    /// Thermal capacity [J/deg C] of the heat sink.
    heat_sink_thermal_capacity: f64,
}

/// Method for rolling over incomplete operations within OperationScheduler.
enum OperationRolloverMethod {
    /// Enqueue incomplete operations for the next time interval.
    _Enqueue,
    /// Drop incomplete operations.
    Drop,
}

/// Schedules operations to send to the simulated CPU.
struct OperationScheduler {
    /// Rate of operations sent to the CPU, scheduled as a function of time.
    rate_schedule: Box<dyn Fn(Seconds) -> Hertz>,
    /// Method for rolling over incomplete operations.
    rollover_method: OperationRolloverMethod,
    /// Number of incomplete operations. Recorded as a float rather than an integer for ease
    /// of use in associated calculations.
    num_operations: f64,
}

impl OperationScheduler {
    fn new(
        rate_schedule: Box<dyn Fn(Seconds) -> Hertz>,
        rollover_method: OperationRolloverMethod,
    ) -> OperationScheduler {
        Self { rate_schedule, rollover_method, num_operations: 0.0 }
    }

    /// Steps from time `t` to `t+dt`, accumulating new operations accordingly.
    fn step(&mut self, t: Seconds, dt: Seconds) {
        if let OperationRolloverMethod::Drop = self.rollover_method {
            self.num_operations = 0.0;
        }
        self.num_operations += (self.rate_schedule)(t) * dt;
    }

    // Marks `num` operations complete.
    fn complete_operations(&mut self, num: f64) {
        assert!(
            num <= self.num_operations,
            "More operations marked complete than were available ({} vs. {})",
            num,
            self.num_operations,
        );
        self.num_operations -= num;
    }
}

struct Simulator {
    /// Test environment.
    test_env: TestEnv,
    /// Proxy to set CPU idle times in CPU driver.
    cpu_proxy: fcpu_ctrl::DeviceProxy,
    /// Proxy to set temperature in CPU temperature driver.
    temperature_ctrl_proxy: ftemperaturecontrol::DeviceProxy,
    /// Proxy to control the fake clock.
    fake_clock_control: ftesting::FakeClockControlProxy,
    /// CPU temperature.
    cpu_temperature: Celsius,
    /// Heat sink temperature.
    heat_sink_temperature: Celsius,
    /// Environment temperature.
    environment_temperature: Celsius,
    /// Simulated time.
    time: Seconds,
    /// Sampling interval.
    sample_interval: Seconds,
    /// Schedules simulated CPU operations.
    op_scheduler: OperationScheduler,
    /// Accumulated idle time on each simulated CPU.
    idle_times: Vec<Nanoseconds>,
    /// Parameters for the simulated CPUs.
    cpu_params: SimulatedCpuParams,
    /// Index of the simulated CPUs' current opp.
    opp_index: usize,
    /// Parameters for the thermal dynamics model.
    thermal_model_params: ThermalModelParams,
}

impl Simulator {
    /// Creates a new Simulator.
    async fn new(
        p: SimulatorParams,
        power_manager_config_path: &str,
        cpu_manager_config_path: &str,
    ) -> Self {
        let test_env = TestEnvBuilder::new()
            .power_manager_node_config_path(power_manager_config_path)
            .cpu_manager_node_config_path(cpu_manager_config_path)
            .build()
            .await;

        // Get sample interval from thermal policy.
        let sample_interval = Self::get_sample_interval(power_manager_config_path);

        let temperature_ctrl_path = "/dev/sys/platform/soc_thermal/control";

        test_env.wait_for_device(temperature_ctrl_path).await;

        let temperature_ctrl_proxy =
            test_env.connect_to_device::<ftemperaturecontrol::DeviceMarker>(temperature_ctrl_path);
        let cpu_proxy = test_env
            .connect_to_first_service_instance(fcpu_ctrl::ServiceMarker)
            .await
            .connect_to_device()
            .unwrap();

        // Make sure CPU numbers are initialized correctly.
        let idle_times = vec![Nanoseconds(0); p.cpu_params.logical_cpu_numbers.len() as usize];
        test_env.set_cpu_stats(idle_times_to_cpu_stats(&idle_times)).await;

        let lifecycle_controller =
            test_env.connect_to_protocol::<fsys2::LifecycleControllerMarker>();

        let (_, cpu_manager_binder_server) = fidl::endpoints::create_endpoints();
        lifecycle_controller
            .start_instance(&format!("./cpu_manager"), cpu_manager_binder_server)
            .await
            .unwrap()
            .unwrap();

        let fake_clock_control = test_env.connect_to_protocol::<ftesting::FakeClockControlMarker>();

        let (deadline_set_event, deadline_set_server) = zx::EventPair::create();

        fake_clock_control.pause().await.expect("failed to pause fake time: FIDL error");

        let () = fake_clock_control
            .add_stop_point(
                &DEADLINE_ID.into(),
                ftesting::DeadlineEventType::Set,
                deadline_set_server,
            )
            .await
            .expect("add_stop_point failed")
            .expect("add_stop_point returned error");

        fake_clock_control
            .resume_with_increments(
                zx::MonotonicDuration::from_millis(1).into_nanos(),
                &ftesting::Increment::Determined(
                    zx::MonotonicDuration::from_millis(1).into_nanos(),
                ),
            )
            .await
            .expect("failed to set fake time scale: FIDL error")
            .expect("failed to set fake time scale: protocol error");

        let (_, power_manager_binder_server) = fidl::endpoints::create_endpoints();
        lifecycle_controller
            .start_instance(&format!("./power_manager"), power_manager_binder_server)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            fasync::OnSignals::new(&deadline_set_event, zx::Signals::EVENTPAIR_SIGNALED)
                .await
                .expect("waiting for timer set failed")
                & !zx::Signals::EVENTPAIR_PEER_CLOSED,
            zx::Signals::EVENTPAIR_SIGNALED
        );

        std::mem::drop(deadline_set_event);

        Self {
            test_env,
            cpu_proxy,
            temperature_ctrl_proxy,
            fake_clock_control,
            cpu_temperature: p.initial_cpu_temperature,
            heat_sink_temperature: p.initial_heat_sink_temperature,
            environment_temperature: p.environment_temperature,
            time: Seconds(0.0),
            sample_interval,
            op_scheduler: p.op_scheduler,
            idle_times,
            opp_index: 0,
            thermal_model_params: p.thermal_model_params,
            cpu_params: p.cpu_params,
        }
    }

    fn get_sample_interval(config_path: &str) -> Seconds {
        let contents = std::fs::read_to_string(config_path).unwrap();
        let json_data: json::Value = serde_json5::from_str(&contents).unwrap();
        for node_config in json_data.as_array().unwrap().iter() {
            if node_config["type"].as_str().unwrap() == "ThermalPolicy" {
                return Seconds(
                    node_config["config"]["controller_params"]["sample_interval"].as_f64().unwrap(),
                );
            }
        }
        panic!("Can not find thermal policy sampling interval in config file")
    }

    async fn get_cpu_opp_index(&self) -> u32 {
        self.cpu_proxy.get_current_operating_point().await.unwrap()
    }

    async fn set_temperature(&self, temperature: f32) {
        let _status =
            self.temperature_ctrl_proxy.set_temperature_celsius(temperature).await.unwrap();
    }

    async fn step_fake_clock(&self) {
        let (deadline_set_event, deadline_set_server) = zx::EventPair::create();

        self.fake_clock_control
            .add_stop_point(
                &DEADLINE_ID.into(),
                ftesting::DeadlineEventType::Set,
                deadline_set_server,
            )
            .await
            .expect("add_stop_point failed")
            .expect("add_stop_point returned error");

        self.fake_clock_control
            .advance(&ftesting::Increment::Determined(
                zx::MonotonicDuration::from_seconds(self.sample_interval.0 as i64).into_nanos(),
            ))
            .await
            .expect("failed to advance fake time: FIDL error")
            .expect("failed to advance fake time: protocol error");

        assert_eq!(
            fasync::OnSignals::new(&deadline_set_event, zx::Signals::EVENTPAIR_SIGNALED)
                .await
                .expect("waiting for timer set failed")
                & !zx::Signals::EVENTPAIR_PEER_CLOSED,
            zx::Signals::EVENTPAIR_SIGNALED
        );

        std::mem::drop(deadline_set_event);
    }

    /// Returns the power consumed by the simulated CPU at the indicated opp and operation
    /// rate.
    fn get_cpu_power(&self, opp_index: usize, operation_rate: Hertz) -> Watts {
        Watts(
            self.cpu_params.capacitance.0
                * self.cpu_params.opps[opp_index].voltage.0.powi(2)
                * operation_rate.0,
        )
    }

    /// Returns the steady-state temperature of the CPU for the provided power consumption.
    /// This assumes all energy consumed is converted into thermal energy.
    fn get_steady_state_cpu_temperature(&self, power: Watts) -> Celsius {
        self.environment_temperature
            + Celsius(
                (1.0 / self.thermal_model_params.cpu_to_heat_sink_thermal_rate
                    + 1.0 / self.thermal_model_params.heat_sink_to_env_thermal_rate)
                    * power.0,
            )
    }

    async fn iterate_n_times(&mut self, n: u32) {
        for _ in 0..n {
            self.step(self.sample_interval).await;
        }
    }

    /// Steps the simulator ahead in time by `dt`.
    async fn step(&mut self, dt: Seconds) {
        self.op_scheduler.step(self.time, dt);

        // `step_cpu` needs to run before `step_thermal_model`, so we know how many operations
        // can actually be completed at the current opp.
        let num_operations_completed = self.step_cpu(dt, self.op_scheduler.num_operations).await;
        self.op_scheduler.complete_operations(num_operations_completed);

        self.step_thermal_model(dt, num_operations_completed).await;
        self.time += dt;
        self.step_fake_clock().await;

        // TODO(b/324650567): This is to wait for CpuManager to finish querying CPU stats. Ideally,
        // we should find a better way to synchronize CpuManager.
        fasync::Timer::new(std::time::Duration::from_millis(20)).await;

        // Get current operating point index by querying the cpu-ctrl driver.
        self.opp_index = self.get_cpu_opp_index().await.try_into().unwrap();
    }

    /// Returns the current opp of the simulated CPU.
    fn get_opp(&self) -> &OperatingPoint {
        &self.cpu_params.opps[self.opp_index]
    }

    /// Steps the thermal model ahead in time by `dt`.
    async fn step_thermal_model(&mut self, dt: Seconds, num_operations: f64) {
        // Define the derivative closure for `rkf45_adaptive`.
        let p = &self.thermal_model_params;
        let dydt = |_t: f64, y: &[f64]| -> Vec<f64> {
            // Aliases for convenience. `0` refers to the CPU and `1` refers to the heat sink,
            // corresponding to their indices in the `temperatures` array passed to
            // rkf45_adaptive.
            let a01 = p.cpu_to_heat_sink_thermal_rate;
            let a1env = p.heat_sink_to_env_thermal_rate;
            let c0 = p.cpu_thermal_capacity;
            let c1 = p.heat_sink_thermal_capacity;

            let power = self.get_cpu_power(self.opp_index, num_operations / dt);
            vec![
                (a01 * (y[1] - y[0]) + power.0) / c0,
                (a01 * (y[0] - y[1]) + a1env * (self.environment_temperature.0 - y[1])) / c1,
            ]
        };

        // Configure `rkf45_adaptive`.
        //
        // The choice for `dt_initial` is currently naive. Given the need, we could try to
        // choose it more intelligently to avoide some discarded time steps in `rkf45_adaptive.`
        //
        // `error_control` is chosen to keep errors near f32 machine epsilon.
        let solver_options = rkf45::AdaptiveOdeSolverOptions {
            t_initial: self.time.0,
            t_final: (self.time + dt).0,
            dt_initial: dt.0,
            error_control: rkf45::ErrorControlOptions::simple(1e-8),
        };

        // Run `rkf45_adaptive`, and update the simulated temperatures.
        let mut temperatures = [self.cpu_temperature.0, self.heat_sink_temperature.0];
        rkf45::rkf45_adaptive(&mut temperatures, &dydt, &solver_options).unwrap();
        self.cpu_temperature = Celsius(temperatures[0]);
        self.set_temperature(self.cpu_temperature.0 as f32).await;
        self.heat_sink_temperature = Celsius(temperatures[1]);
    }

    /// Steps the simulated CPU ahead by `dt`, updating `self.idle_times` and returning the
    /// number of operations completed.
    async fn step_cpu(&mut self, dt: Seconds, num_operations_requested: f64) -> f64 {
        let frequency = self.get_opp().frequency;
        let num_operations_completed = f64::min(
            num_operations_requested,
            frequency * dt * self.cpu_params.logical_cpu_numbers.len() as f64,
        );

        let total_cpu_time = num_operations_completed / frequency;
        let active_time_per_core =
            total_cpu_time.div_scalar(self.cpu_params.logical_cpu_numbers.len() as f64);

        // Calculation of `num_operations_completed` should guarantee this condition.
        assert!(active_time_per_core <= dt);

        let idle_time_per_core = dt - active_time_per_core;
        self.idle_times.iter_mut().for_each(|x| *x += idle_time_per_core.into());
        self.test_env.set_cpu_stats(idle_times_to_cpu_stats(&self.idle_times)).await;

        num_operations_completed
    }

    async fn destroy(&mut self) {
        info!("Destroying Simulator");
        self.test_env.destroy().await;
    }
}

fn idle_times_to_cpu_stats(idle_times: &Vec<Nanoseconds>) -> fkernel::CpuStats {
    let mut per_cpu_stats = Vec::new();
    for (i, idle_time) in idle_times.iter().enumerate() {
        per_cpu_stats.push(fkernel::PerCpuStats {
            cpu_number: Some(i as u32),
            flags: None,
            idle_time: Some(idle_time.0),
            reschedules: None,
            context_switches: None,
            irq_preempts: None,
            yields: None,
            ints: None,
            timer_ints: None,
            timers: None,
            page_faults: None,
            exceptions: None,
            syscalls: None,
            reschedule_ipis: None,
            generic_ipis: None,
            ..Default::default()
        });
    }

    fkernel::CpuStats {
        actual_num_cpus: idle_times.len() as u64,
        per_cpu_stats: Some(per_cpu_stats),
    }
}

/// Consistent with CpuControlHandler configs / opps retrieved from `cpu-ctrl` driver.
fn default_cpu_params() -> SimulatedCpuParams {
    SimulatedCpuParams {
        logical_cpu_numbers: vec![0, 1, 2, 3],
        opps: vec![
            OperatingPoint { frequency: Hertz(2.0e9), voltage: Volts(1.0) },
            OperatingPoint { frequency: Hertz(1.5e9), voltage: Volts(0.8) },
            OperatingPoint { frequency: Hertz(1.2e9), voltage: Volts(0.7) },
        ],
        capacitance: Farads(150.0e-12),
    }
}

fn default_thermal_model_params() -> ThermalModelParams {
    ThermalModelParams {
        cpu_to_heat_sink_thermal_rate: 0.14,
        heat_sink_to_env_thermal_rate: 0.035,
        cpu_thermal_capacity: 0.003,
        heat_sink_thermal_capacity: 28.0,
    }
}

// Verifies that the simulated CPU follows expected fast-scale thermal dynamics.
#[fuchsia::test]
async fn test_fast_scale_thermal_dynamics() {
    let power_manager_config_path = "/pkg/cpu_thermal_model_test/power_manager_node_config.json5";
    let cpu_manager_config_path = "/pkg/cpu_thermal_model_test/cpu_manager_node_config.json5";

    // Use a fixed operation rate for this test.
    let operation_rate = Hertz(3e9);

    let mut thermal_test_simulator = Simulator::new(
        SimulatorParams {
            thermal_model_params: default_thermal_model_params(),
            cpu_params: default_cpu_params(),
            op_scheduler: OperationScheduler::new(
                Box::new(move |_| operation_rate),
                OperationRolloverMethod::Drop,
            ),
            initial_cpu_temperature: Celsius(30.0),
            initial_heat_sink_temperature: Celsius(30.0),
            environment_temperature: Celsius(22.0),
        },
        power_manager_config_path,
        cpu_manager_config_path,
    )
    .await;

    // After ten seconds with no intervention by the thermal policy, the CPU temperature should
    // be very close to the value dictated by the fast-scale thermal dynamics.
    thermal_test_simulator.iterate_n_times(10).await;
    let power = thermal_test_simulator.get_cpu_power(0, operation_rate);
    let target_temp = thermal_test_simulator.heat_sink_temperature.0
        + power.0 / thermal_test_simulator.thermal_model_params.cpu_to_heat_sink_thermal_rate;
    assert_near!(target_temp, thermal_test_simulator.cpu_temperature.0, 1e-3);

    thermal_test_simulator.destroy().await;
}

// Tests that under a constant operation rate, the thermal policy drives the average CPU
// temperature to the target temperature.
#[fuchsia::test]
async fn test_average_temperature() {
    let power_manager_config_path = "/pkg/cpu_thermal_model_test/power_manager_node_config.json5";
    let cpu_manager_config_path = "/pkg/cpu_thermal_model_test/cpu_manager_node_config.json5";

    // Use a fixed operation rate for this test.
    let operation_rate = Hertz(3e9);

    // Consistent with target temperature in thermal policy (specified in config file).
    let target_temperature = Celsius(85.0);

    let mut thermal_test_simulator = Simulator::new(
        SimulatorParams {
            thermal_model_params: default_thermal_model_params(),
            cpu_params: default_cpu_params(),
            op_scheduler: OperationScheduler::new(
                Box::new(move |_| operation_rate),
                OperationRolloverMethod::Drop,
            ),
            initial_cpu_temperature: Celsius(80.0),
            initial_heat_sink_temperature: Celsius(80.0),
            environment_temperature: Celsius(75.0),
        },
        power_manager_config_path,
        cpu_manager_config_path,
    )
    .await;

    // Make sure that for the operation rate we're using, the steady-state temperature for the
    // highest-power opp is above the target temperature, while the one for the
    // lowest-power opp is below it.
    assert!(
        thermal_test_simulator.get_steady_state_cpu_temperature(
            thermal_test_simulator.get_cpu_power(0, operation_rate)
        ) > target_temperature
    );
    assert!(
        thermal_test_simulator.get_steady_state_cpu_temperature(
            thermal_test_simulator
                .get_cpu_power(thermal_test_simulator.cpu_params.opps.len() - 1, operation_rate)
        ) < target_temperature
    );

    // Warm up for 8 minutes of simulated time.
    thermal_test_simulator.iterate_n_times(480).await;

    // Calculate the average CPU temperature over the next 30 iterations, and ensure that it's
    // close to the target temperature.
    let average_temperature = {
        let mut cumulative_sum = 0.0;
        for _ in 0..30 {
            thermal_test_simulator.iterate_n_times(1).await;
            cumulative_sum += thermal_test_simulator.cpu_temperature.0;
        }
        cumulative_sum / 30.0
    };
    assert_near!(average_temperature, target_temperature.0, 0.2);
    thermal_test_simulator.destroy().await;
}

// Tests for a bug that led to jitter in opp selection at max load.
//
// CpuControlHandler was originally implemented to estimate the operation rate in the upcoming
// cycle as the operation rate over the previous cycle, even if the previous rate was maximal.
// This underpredicted the new operation rate when the CPU was saturated.
//
// For example, suppose a 4-core CPU operated at 1.5 GHz over the previous cycle. If it was
// saturated, its operation rate was 6.0 GHz. If we raise the clock speed to 2GHz and the CPU
// remains saturated, we will have underpredicted its operation rate by 25%.
//
// This underestimation manifested as unwanted jitter between opps. After transitioning from
// P0 to P1, for example, the available power required to select P0 would drop by the ratio of
// frequencies, f1/f0. This made an immediate transition back to P0 very likely.
//
// Note that since the CPU temperature immediately drops when its clock speed is lowered, this
// behavior of dropping clock speed for a single cycle may occur for good reason. To isolate the
// undesired behavior in this test, we use an extremely large time constant. Doing so mostly
// eliminates the change in filtered temperature in the cycles immediately following a opp
// transition.
#[fuchsia::test]
async fn test_no_jitter_at_max_load() {
    // Use a very large filter time constant: 1 deg raw --> 0.001 deg filtered in the first
    // cycle after a change.
    let power_manager_config_path =
        "/pkg/cpu_thermal_model_no_jitter_test/power_manager_node_config.json5";
    let cpu_manager_config_path =
        "/pkg/cpu_thermal_model_no_jitter_test/cpu_manager_node_config.json5";

    // Choose an operation rate that induces max load at highest frequency.
    let operation_rate = Hertz(8.0e9);

    let mut thermal_test_simulator = Simulator::new(
        SimulatorParams {
            thermal_model_params: default_thermal_model_params(),
            cpu_params: default_cpu_params(),
            op_scheduler: OperationScheduler::new(
                Box::new(move |_| operation_rate),
                OperationRolloverMethod::Drop,
            ),
            initial_cpu_temperature: Celsius(75.0),
            initial_heat_sink_temperature: Celsius(75.0),
            environment_temperature: Celsius(75.0),
        },
        power_manager_config_path,
        cpu_manager_config_path,
    )
    .await;

    // Run the simulation (up to 1 hour simulated time) until the CPU transitions to a lower
    // clock speed.
    let max_iterations = 3600;
    let mut throttling_started = false;
    for _ in 0..max_iterations {
        thermal_test_simulator.iterate_n_times(1).await;
        if thermal_test_simulator.opp_index > 0 {
            assert_eq!(thermal_test_simulator.opp_index, 1, "Should have transitioned to opp 1.");
            throttling_started = true;
            break;
        }
    }
    assert!(
        throttling_started,
        "CPU throttling did not begin within {} iterations",
        max_iterations
    );

    // Iterated one more time, and make sure the clock speed is still reduced.
    thermal_test_simulator.iterate_n_times(1).await;
    assert_ne!(thermal_test_simulator.opp_index, 0);

    thermal_test_simulator.destroy().await;
}
