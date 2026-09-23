// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::subsystems::prelude::*;
use anyhow::{Context, Result, anyhow};
use assembly_constants::BoardFeature;

use assembly_config_capabilities::{Config, ConfigValueType};
use assembly_config_schema::platform_settings::timekeeper_config::TimekeeperConfig;

pub(crate) struct TimekeeperSubsystem;
impl DefineSubsystemConfiguration<TimekeeperConfig> for TimekeeperSubsystem {
    fn define_configuration(
        context: &ConfigurationContext<'_>,
        config: &TimekeeperConfig,
        builder: &mut dyn ConfigurationBuilder,
    ) -> Result<()> {
        // This is an experimental feature that we want to deploy with care.
        // We originally wanted to deploy on eng builds as well, but it proved
        // to be confusing for debugging.
        //
        // See: b/308199171
        let utc_start_at_startup =
            context.board_config.provides_feature(BoardFeature::UtcStartAtStartup);

        // Soft crypto boards don't yet have crypto support, so we exit timekeeper
        // early instead of having it crash repeatedly.
        //
        // See: b/299320231
        let early_exit = context.board_config.provides_feature(BoardFeature::SoftCrypto);

        // Some e2e tests need to change Timekeeper behavior at runtime. This setting
        // allows Timekeeper to serve an endpoint for such runtime behavior changes.
        // Only eng builds are allowed to have this feature.
        let serve_test_protocols = config.serve_test_protocols;
        let use_persistent_storage = config.use_persistent_storage;

        // See:
        // https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0115_build_types?hl=en#definitions
        if serve_test_protocols && *context.build_type != BuildType::Eng {
            return Err(anyhow!(
                "`serve_test_protocols==true` is only allowed in `eng` builds, see RFC 0115"
            ));
        }

        // Gives Timekeeper some mutable persistent storage ("data").
        //
        // `serve_test_protocols` is gating this capability because historically it also was
        // used to gate persistent storage. `use_persistent_storage` is a direct setting.
        if serve_test_protocols || use_persistent_storage {
            builder.platform_bundle("timekeeper_persistence")?;
        }

        let has_hrtimer = context.board_config.provides_feature(BoardFeature::Hrtimer);

        // If set, Timekeeper will serve `fuchsia.time.alarms` and will connect
        // to the appropriate hardware device to do so.
        //
        // At the moment the flag is set only if all conditions are met, i.e. the functionality
        // is requested, and the underlying driver is available.
        //
        // The same information is offered as a config capability to other components.
        let serve_fuchsia_time_alarms = config.serve_fuchsia_time_alarms && has_hrtimer;
        builder.set_config_capability(
            "fuchsia.time.config.HasWakeAlarms",
            //TODO: b/416081776 - replace `true` with serve_fuchsia_time_alarms.
            Config::new(ConfigValueType::Bool, true.into()),
        )?;

        // Adds hrtimer routing only for boards that have hardware support for
        // doing so.
        if serve_fuchsia_time_alarms {
            // For all devices.
            builder.platform_bundle("timekeeper_wake_alarms")?;
        }

        // Always on counter is used instead of a persistent RTC on some platforms.
        let has_always_on_counter =
            context.board_config.provides_feature(BoardFeature::AlwaysOnCounter);

        // The always on counter is useless without persistent storage to persist the
        // counter values as would be done in an RTC chip.
        let use_always_on_counter = has_always_on_counter && use_persistent_storage;

        // Allows Timekeeper to short-circuit RTC driver detection at startup.
        let has_real_time_clock =
            context.board_config.provides_feature(BoardFeature::RealTimeClock)
                || use_always_on_counter;

        // Refer to //src/sys/time/timekeeper/config.shard.cml
        // for details.
        builder.set_config_capability(
            "fuchsia.time.config.WritableUTCTime",
            Config::new(ConfigValueType::Bool, config.serve_fuchsia_time_external_adjust.into()),
        )?;

        let mut config_builder = builder
            .package("timekeeper")
            .component("meta/timekeeper.cm")
            .context("while finding the timekeeper component")?;

        // Refer to //src/sys/time/timekeeper/config.shard.cml for details.
        config_builder
            .field("disable_delays", false)?
            .field("oscillator_error_std_dev_ppm", 15)?
            .field("max_frequency_error_ppm", 30)?
            .field(
                "primary_time_source_url",
                "fuchsia-pkg://fuchsia.com/httpsdate-time-source-pull#meta/httpsdate_time_source.cm",
            )?
            .field("monitor_time_source_url", "")?
            .field("initial_frequency_ppm", 1_000_000)?
            .field("primary_uses_pull", true)?
            .field("monitor_uses_pull", false)?
            .field("back_off_time_between_pull_samples_sec",
                config.back_off_time_between_pull_samples_sec)?
            .field("first_sampling_delay_sec", config.first_sampling_delay_sec)?
            .field("utc_start_at_startup", utc_start_at_startup)?
            .field("early_exit", early_exit)?
            .field("power_topology_integration_enabled", config.power_topology_integration_enabled)?
            .field("serve_test_protocols", serve_test_protocols)?
            // Should this now be removed in favor of WritableUTCTime above?
            .field("serve_fuchsia_time_external_adjust", config.serve_fuchsia_time_external_adjust)?
            .field("has_real_time_clock", has_real_time_clock)?
            .field("has_always_on_counter", use_always_on_counter)?
            .field("utc_start_at_startup_when_invalid_rtc", config.utc_start_at_startup_when_invalid_rtc)?
            .field("utc_max_allowed_delta_past_sec", config.utc_max_allowed_delta_past_sec)?
            .field("utc_max_allowed_delta_future_sec", config.utc_max_allowed_delta_future_sec)?
            .field("use_connectivity", config.use_connectivity)?
            .field("min_utc_reference_to_backstop_diff_minutes", config.min_utc_reference_to_backstop_diff_minutes)?
            .field(
                "min_acceptable_boot_time_for_utc_update_minutes",
                config.min_acceptable_boot_time_for_utc_update_minutes,
            )?
            .field("periodic_rtc_update_interval_minutes", config.periodic_rtc_update.into_structured_config_value())?
            .field("rtc_allow_setting_past_utc", String::from(config.rtc_allow_setting_past_utc))?
            .field("serve_fuchsia_time_alarms", serve_fuchsia_time_alarms)?;

        let mut time_source_config_builder = builder
            .package("httpsdate-time-source-pull")
            .component("meta/httpsdate_time_source.cm")
            .context("while finding the time source component")?;

        // Refer to //src/sys/time/httpsdate_time_source/meta/service.cml
        // for details.
        time_source_config_builder
            .field("use_fake_sampler", false)?
            .field("https_timeout_sec", 10)?
            .field("standard_deviation_bound_percentage", 30)?
            .field("first_rtt_time_factor", 5)?
            .field("use_pull_api", true)?
            .field("max_attempts_urgency_low", 3)?
            .field("num_polls_urgency_low", 7)?
            .field("max_attempts_urgency_medium", 3)?
            .field("num_polls_urgency_medium", 5)?
            .field("max_attempts_urgency_high", 3)?
            .field("num_polls_urgency_high", 3)?
            .field("time_source_endpoint_url", &*config.time_source_endpoint_url)?
            // timekeeper will set this to true for monitor time sources.
            .field("is_monitor_time_source", false)?;

        if has_hrtimer && matches!(context.build_type, BuildType::Eng) {
            match context.feature_set_level {
                FeatureSetLevel::Embeddable | FeatureSetLevel::Bootstrap => {}
                FeatureSetLevel::Utility | FeatureSetLevel::Standard => {
                    builder.platform_bundle("hrtimer-ctl")?;
                }
            }
        }
        Ok(())
    }
}
