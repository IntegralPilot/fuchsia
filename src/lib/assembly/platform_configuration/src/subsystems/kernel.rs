// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::subsystems::prelude::*;
use anyhow::{Context, anyhow};
use assembly_config_schema::board_config::SerialMode;
use assembly_config_schema::platform_settings::kernel_config::{
    MemoryReclamationStrategy, OOMBehavior, OOMRebootTimeout, PagetableEvictionPolicy,
    PlatformKernelConfig,
};
use assembly_constants::{BoardFeature, BootfsDestination, FileEntry, KernelArg};
use camino::Utf8Path;
use serde_json::Value;
use std::fs::File;
pub(crate) struct KernelSubsystem;

impl DefineSubsystemConfiguration<PlatformKernelConfig> for KernelSubsystem {
    fn define_configuration(
        context: &ConfigurationContext<'_>,
        kernel_config: &PlatformKernelConfig,
        builder: &mut dyn ConfigurationBuilder,
    ) -> anyhow::Result<()> {
        match (&context.build_type, &kernel_config.oom.behavior) {
            (_, OOMBehavior::Reboot { timeout: OOMRebootTimeout::Normal }) => {}
            (&BuildType::Eng, OOMBehavior::Reboot { timeout: OOMRebootTimeout::Low }) => {
                builder.platform_bundle("kernel_oom_reboot_timeout_low")?
            }
            (&BuildType::Eng, OOMBehavior::JobKill) => {
                builder.platform_bundle("kernel_oom_behavior_jobkill")?
            }
            (&BuildType::Eng, OOMBehavior::Disable) => {
                builder.platform_bundle("kernel_oom_behavior_disable")?
            }
            (&BuildType::UserDebug | &BuildType::User, _) => {
                anyhow::bail!("'kernel.oom.behavior' can only be set on 'build_type=\"eng\"");
            }
        }
        if let Some(eviction_delay_ms) = kernel_config.oom.eviction_delay_ms {
            builder.kernel_arg(KernelArg::OomEvictionDelayMs(eviction_delay_ms));
        }
        if !kernel_config.oom.evict_with_min_target {
            // the default is true
            builder.kernel_arg(KernelArg::OomEvictWithMinTarget(false));
        }
        if let Some(eviction_delta_at_oom_mb) = kernel_config.oom.eviction_delta_at_oom_mb {
            builder.kernel_arg(KernelArg::OomEvictionDeltaAtOomMib(eviction_delta_at_oom_mb));
        }
        if kernel_config.oom.evict_at_warning {
            builder.kernel_arg(KernelArg::OomEvictAtWarning(true));
        }
        if kernel_config.oom.evict_continuous {
            builder.kernel_arg(KernelArg::OomEvictContinuous(true));
        }
        if let Some(outofmemory_mb) = kernel_config.oom.out_of_memory_mb {
            builder.kernel_arg(KernelArg::OomOutOfMemoryMib(outofmemory_mb));
        }
        if let Some(critical_mb) = kernel_config.oom.critical_mb {
            builder.kernel_arg(KernelArg::OomCriticalMib(critical_mb));
        }
        if let Some(warning_mb) = kernel_config.oom.warning_mb {
            builder.kernel_arg(KernelArg::OomWarningMib(warning_mb));
        }
        if let Some(imminent_oom_delta_mb) = kernel_config.oom.imminent_oom_delta_mb {
            builder.kernel_arg(KernelArg::OomImminentOomDeltaMib(imminent_oom_delta_mb));
        }
        if let Some(debounce_mb) = kernel_config.oom.debounce_mb {
            builder.kernel_arg(KernelArg::OomDebounceMib(debounce_mb));
        }
        if let Some(hysteresis_sec) = kernel_config.oom.hysteresis_seconds {
            builder.kernel_arg(KernelArg::OomHysteresisSeconds(hysteresis_sec));
        }
        if kernel_config.oom.experimental_expand_memory_stall {
            builder.kernel_arg(KernelArg::OomExperimentalExpandMemoryStall(true));
        }
        match (&context.board_config.kernel.serial_mode, &context.build_type) {
            (SerialMode::NoOutput, &BuildType::User) => {
                builder.kernel_arg(KernelArg::Serial("none".to_string()))
            }
            (SerialMode::Legacy, &BuildType::Eng) => {
                builder.platform_bundle("kernel_serial_legacy")?
            }
            (SerialMode::NoOutput, &BuildType::UserDebug | &BuildType::Eng) => {}
            (SerialMode::Legacy, &BuildType::UserDebug | &BuildType::User) => {}
        }

        if let Some(serial) = &context.board_config.kernel.serial
            && context.build_type == &BuildType::Eng
        {
            builder.kernel_arg(KernelArg::Serial(serial.to_string()));
        }

        if kernel_config.lru_memory_compression && !kernel_config.memory_compression {
            anyhow::bail!("'lru_memory_compression' can only be enabled with 'memory_compression'");
        }
        if kernel_config.memory_compression {
            builder.platform_bundle("kernel_anonymous_memory_compression")?;
        }
        if kernel_config.lru_memory_compression {
            builder.platform_bundle("kernel_anonymous_memory_compression_eager_lru")?;
        }

        // If the board supports the PMM checker, and this is an eng build-type
        // build, enable the pmm checker.
        if context.board_config.provides_feature(BoardFeature::PmmChecker)
            && context.board_config.provides_feature(BoardFeature::PmmCheckerAuto)
        {
            anyhow::bail!(
                "Board provides conflicting features of 'fuchsia::pmm_checker' and 'fuchsia::pmm_checker_auto'"
            );
        }
        if context.board_config.provides_feature(BoardFeature::PmmChecker)
            && context.build_type == &BuildType::Eng
        {
            builder.platform_bundle("kernel_pmm_checker_enabled")?;
        } else if context.board_config.provides_feature(BoardFeature::PmmCheckerAuto)
            && context.build_type == &BuildType::Eng
        {
            builder.platform_bundle("kernel_pmm_checker_enabled_auto")?;
        }

        if context.board_config.kernel.contiguous_physical_pages {
            builder.platform_bundle("kernel_contiguous_physical_pages")?;
        }

        if context.board_config.kernel.scheduler_prefer_little_cpus {
            builder.kernel_arg(KernelArg::SchedulerPreferLittleCpus(true));
        }

        if kernel_config.scheduler_enable_new_wakeup_accounting {
            builder.kernel_arg(KernelArg::SchedulerEnableNewWakeupAccounting(true));
        }

        if !context.board_config.kernel.arm64_event_stream_enable {
            builder.platform_bundle("kernel_arm64_event_stream_disable")?;
        }

        if context.board_config.kernel.quiet_early_boot {
            anyhow::ensure!(
                context.build_type == &BuildType::Eng,
                "'quiet_early_boot' can only be enabled in 'eng' builds"
            );
            builder.kernel_arg(KernelArg::PhysVerbose(false))
        }

        match kernel_config.memory_reclamation_strategy {
            MemoryReclamationStrategy::Balanced => {
                // Use the kernel defaults.
            }
            MemoryReclamationStrategy::Eager => {
                builder.platform_bundle("kernel_page_scanner_aging_fast")?;
            }
        }

        if context.board_config.kernel.halt_on_panic {
            anyhow::ensure!(
                context.build_type == &BuildType::Eng,
                "'kernel.halt-on-panic' can only be enabled in 'eng' builds"
            );
            builder.kernel_arg(KernelArg::HaltOnPanic(true))
        }

        if context.board_config.kernel.allow_debug_uart_suspend {
            builder.kernel_arg(KernelArg::AllowDebugUartSuspend(true));
        }

        if let Some(page_scanner) = &kernel_config.page_scanner {
            match page_scanner.page_table_eviction_policy {
                PagetableEvictionPolicy::Never => {
                    builder.platform_bundle("kernel_page_table_eviction_never")?
                }
                PagetableEvictionPolicy::OnRequest => {
                    builder.platform_bundle("kernel_page_table_eviction_on_request")?
                }
                PagetableEvictionPolicy::Always => {}
            }

            if page_scanner.disable_at_boot {
                builder.kernel_arg(KernelArg::PageScannerStartAtBoot(false));
            }

            if page_scanner.disable_eviction {
                builder.kernel_arg(KernelArg::PageScannerEnableEviction(false));
            }

            builder.kernel_arg(KernelArg::PageScannerZeroPageScanCount(
                page_scanner.zero_page_scans_per_second.clone(),
            ));
        }

        if let Some(aslr_entropy_bits) = kernel_config.aslr_entropy_bits {
            builder.kernel_arg(KernelArg::AslrEntropyBits(aslr_entropy_bits));
        }

        if kernel_config.cprng.seed_require_jitterentropy {
            builder.kernel_arg(KernelArg::CprngSeedRequireJitterEntropy(true))
        }

        if kernel_config.cprng.seed_require_cmdline {
            builder.kernel_arg(KernelArg::CprngSeedRequireCmdline(true))
        }

        if kernel_config.cprng.reseed_require_jitterentropy {
            builder.kernel_arg(KernelArg::CprngReseedRequireJitterEntropy(true))
        }

        if let Some(memory_limit_mb) = kernel_config.memory_limit_mb {
            builder.kernel_arg(KernelArg::MemoryLimitMib(memory_limit_mb));
        }

        if let Some(ktrace_bufsize) = kernel_config.ktrace.bufsize {
            builder.kernel_arg(KernelArg::KtraceBufsize(ktrace_bufsize));
        }

        if let Some(bs) = kernel_config.jitterentropy_bs {
            builder.kernel_arg(KernelArg::JitterentropyBs(bs));
        }

        if let Some(bc) = kernel_config.jitterentropy_bc {
            builder.kernel_arg(KernelArg::JitterentropyBc(bc));
        }

        if let Some(ml) = kernel_config.jitterentropy_ml {
            builder.kernel_arg(KernelArg::JitterentropyMl(ml));
        }

        if let Some(ll) = kernel_config.jitterentropy_ll {
            builder.kernel_arg(KernelArg::JitterentropyLl(ll));
        }

        if let Some(entropy_per_1000_bytes) = kernel_config.jitterentropy_entropy_per_1000_bytes {
            builder.kernel_arg(KernelArg::JitterentropyEntropyPer1000Bytes(entropy_per_1000_bytes));
        }

        if kernel_config.heap.enable_virtually_managed {
            builder.kernel_arg(KernelArg::EnableVirtualHeap(true));
        }

        if let Some(max_size_mb) = kernel_config.heap.max_size_mb {
            builder.kernel_arg(KernelArg::HeapMaxSizeMib(max_size_mb));
        }

        // Read a thread roles file as JSON5 and write it as JSON to avoid build time errors.
        let gendir = context.get_gendir().context("Getting gendir for kernel subsystem")?;
        for thread_roles_file in &context.board_config.configuration.thread_roles {
            let file = File::open(thread_roles_file).with_context(|| {
                format!("Failed to open thread roles file: {thread_roles_file}")
            })?;
            let json_value: Value = serde_json5::from_reader(&file).with_context(|| {
                format!("Thread roles file is not a json5 file: {thread_roles_file}")
            })?;
            let filename = thread_roles_file.file_name().ok_or_else(|| {
                anyhow!("Thread roles file doesn't have a filename: {}", thread_roles_file)
            })?;
            let json_filename = Utf8Path::new(filename).with_extension("profiles");
            let json_path = gendir.join(&json_filename);
            let json_file = File::create(&json_path)
                .with_context(|| format!("Failed to create new thread roles file: {json_path}"))?;
            serde_json::to_writer(json_file, &json_value).with_context(|| {
                format!("Failed to write to new thread roles file: {json_path}")
            })?;
            builder
                .bootfs()
                .file(FileEntry {
                    source: json_path,
                    destination: BootfsDestination::ThreadRoles(json_filename.to_string()),
                })
                .with_context(|| format!("Adding thread roles file: {thread_roles_file}"))?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::CompletedConfiguration;
    use crate::subsystems::kernel::KernelSubsystem;
    use crate::subsystems::{ConfigurationBuilderImpl, ConfigurationContext, FeatureSetLevel};
    use assembly_config_schema::{BoardConfig, BoardProvidedConfig, BuildType};
    use camino::Utf8PathBuf;
    use std::fs;
    use tempfile::tempdir;

    fn build_with_platform_kernel_config(
        platform_kernel_config: PlatformKernelConfig,
    ) -> CompletedConfiguration {
        build_with_board_info(platform_kernel_config, Default::default())
    }

    fn build_with_board_info(
        platform_kernel_config: PlatformKernelConfig,
        board_config: BoardConfig,
    ) -> CompletedConfiguration {
        let context = ConfigurationContext {
            feature_set_level: &FeatureSetLevel::Standard,
            build_type: &BuildType::Eng,
            board_config: &board_config,
            gendir: Default::default(),
            resource_dir: Default::default(),
            developer_only_options: Default::default(),
        };
        let mut builder: ConfigurationBuilderImpl = Default::default();
        let result =
            KernelSubsystem::define_configuration(&context, &platform_kernel_config, &mut builder);
        assert!(result.is_ok());
        builder.build()
    }

    #[test]
    fn test_define_configuration() {
        let completed_config = build_with_platform_kernel_config(Default::default());
        assert!(completed_config.kernel_args.is_empty());
    }

    #[test]
    fn test_define_configuration_aslr() {
        let completed_config = build_with_platform_kernel_config(PlatformKernelConfig {
            aslr_entropy_bits: Some(12),
            ..Default::default()
        });
        assert!(completed_config.kernel_args.contains("aslr.entropy_bits=12"));
    }

    #[test]
    fn test_define_configuration_no_aslr() {
        let completed_config = build_with_platform_kernel_config(Default::default());
        assert!(completed_config.kernel_args.is_empty());
    }

    #[test]
    fn test_define_memory_limit() {
        let completed_config = build_with_platform_kernel_config(PlatformKernelConfig {
            memory_limit_mb: Some(12),
            ..Default::default()
        });
        assert!(completed_config.kernel_args.contains("kernel.memory-limit-mb=12"));
    }

    #[test]
    fn test_thread_roles_valid_path() {
        let temp_dir = tempdir().unwrap();
        let roles_path =
            Utf8PathBuf::from_path_buf(temp_dir.path().join("test_file.json5")).unwrap();
        fs::write(&roles_path, "{ key: 'value', /* comment */ }").unwrap();
        let board_config: BoardConfig = BoardConfig {
            configuration: BoardProvidedConfig {
                thread_roles: vec![roles_path],
                ..Default::default()
            },
            ..Default::default()
        };
        let completed_config = build_with_board_info(Default::default(), board_config);
        assert_eq!(completed_config.bootfs.files.map.entries.len(), 1);
        let file_entry_key = BootfsDestination::ThreadRoles("test_file.profiles".to_string());
        let source_merkle_pair = completed_config
            .bootfs
            .files
            .map
            .entries
            .get(&file_entry_key)
            .expect("File not found in bootfs with the expected destination key.");
        let source_path = &source_merkle_pair.source;
        let file_content = fs::read_to_string(source_path).unwrap();
        let parsed_json: Value = serde_json::from_str(&file_content).unwrap();
        let expected_json: Value = serde_json::from_str(r#"{"key":"value"}"#).unwrap();
        assert_eq!(parsed_json, expected_json);
    }

    #[test]
    fn test_thread_roles_file_not_found() {
        let temp_dir = tempdir().unwrap();
        let roles_path =
            Utf8PathBuf::from_path_buf(temp_dir.path().join("test_file.json5")).unwrap();
        let board_config: BoardConfig = BoardConfig {
            configuration: BoardProvidedConfig {
                thread_roles: vec![roles_path.clone()],
                ..Default::default()
            },
            ..Default::default()
        };
        let context = ConfigurationContext {
            feature_set_level: &FeatureSetLevel::Standard,
            build_type: &BuildType::Eng,
            board_config: &board_config,
            gendir: Default::default(),
            resource_dir: Default::default(),
            developer_only_options: Default::default(),
        };
        let mut builder: ConfigurationBuilderImpl = Default::default();
        let result =
            KernelSubsystem::define_configuration(&context, &Default::default(), &mut builder);
        assert!(result.is_err());
        let error_message = result.unwrap_err().to_string();
        assert!(error_message.contains("Failed to open thread roles file"));
        assert!(error_message.contains(roles_path.as_str()));
    }

    #[test]
    fn test_thread_roles_invalid_json5() {
        let temp_dir = tempdir().unwrap();
        let roles_path = Utf8PathBuf::from_path_buf(temp_dir.path().join("invalid.json5")).unwrap();
        fs::write(&roles_path, "{ invalid json file, }").unwrap();
        let board_config: BoardConfig = BoardConfig {
            configuration: BoardProvidedConfig {
                thread_roles: vec![roles_path.clone()],
                ..Default::default()
            },
            ..Default::default()
        };
        let context = ConfigurationContext {
            feature_set_level: &FeatureSetLevel::Standard,
            build_type: &BuildType::Eng,
            board_config: &board_config,
            gendir: Default::default(),
            resource_dir: Default::default(),
            developer_only_options: Default::default(),
        };
        let mut builder: ConfigurationBuilderImpl = Default::default();
        let result =
            KernelSubsystem::define_configuration(&context, &Default::default(), &mut builder);
        assert!(result.is_err());
        let error_message = result.unwrap_err().to_string();
        assert!(error_message.contains("Thread roles file is not a json5 file"));
        assert!(error_message.contains(roles_path.as_str()));
    }
}
