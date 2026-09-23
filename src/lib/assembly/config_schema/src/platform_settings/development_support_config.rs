// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{BuildType, FeatureSetLevel};
use anyhow::Result;
use assembly_container::WalkPaths;
use camino::Utf8PathBuf;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Platform configuration options for enabling development support.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema, WalkPaths)]
#[serde(default, deny_unknown_fields)]
pub struct DevelopmentSupportConfig {
    /// Override the build-type enablement of development support, to include
    /// development support in userdebug which doesn't have full development
    /// access.
    /// If nothing is provided, a reasonable default is used based on the build type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,

    // Whether to use vsock based development connection.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub vsock_development: bool,

    /// Path to a file containing ssh keys that are authorized to connect to the
    /// device.
    #[walk_paths]
    #[schemars(schema_with = "crate::option_path_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authorized_ssh_keys_path: Option<Utf8PathBuf>,

    /// Path to a file containing CA certs that are trusted roots for signed ssh
    /// keys that are authorized to connect to the device.
    #[walk_paths]
    #[schemars(schema_with = "crate::option_path_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authorized_ssh_ca_certs_path: Option<Utf8PathBuf>,

    /// Whether to include sl4f.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub include_sl4f: bool,

    /// Include the bin/clock program on the target to get the monotonic time
    /// from the device. TODO(b/309452964): Remove once e2e tests use:
    ///   `ffx target get-time`
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub include_bin_clock: bool,

    /// Override netsvc inclusion on the target.
    ///
    /// Follows the same resolution as `enabled` if absent.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub include_netsvc: bool,

    /// Override tracing inclusion on the target.
    ///
    // TODO(https://fxbug.dev/461878941): Soft transition to remove this in favor of
    // [`TracingConfig`].
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub include_tracing: bool,

    /// Enable the netboot feature of the netsvc
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub enable_netsvc_netboot: bool,

    /// Tools to enable along with development support
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub tools: ToolsConfig,

    /// Whether to include the bootstrap testing framework which will allow running tests in a
    /// bringup-like environment using the run-test-suite command line tool.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub include_bootstrap_testing_framework: bool,

    /// Enable userboot.next for running a boot-time test.
    ///
    /// Only valid on eng builds.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub enable_userboot_next_component_manager: bool,

    /// Configure tracing. Note: [`DevelopmentSupportConfig::include_tracing`] must be `true` for
    /// tracing to be available.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub tracing: TracingConfig,

    /// Configure Heapdump memory profiling.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub heapdump: HeapdumpConfig,
}

/// Platform-provided tools for development and debugging.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ToolsConfig {
    /// Tools for audio.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub audio: AudioToolsConfig,

    /// Tools for connectivity.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub connectivity: ConnectivityToolsConfig,

    /// Tools for storage.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub storage: StorageToolsConfig,

    /// Tools for display.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub display: DisplayToolsConfig,
}

/// Platform-provided tools for development and debugging of display.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct DisplayToolsConfig {
    /// Include tools for display driver development and debugging, such as:
    ///   - 'display-tool'
    ///   - 'display-tweak'
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub driver_tools: bool,
}

/// Platform-provided tools for development and debugging of audio.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct AudioToolsConfig {
    /// Include tools for audio driver development.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub driver_tools: bool,

    /// Include tools for debugging the audio_core service, such as:
    ///   - 'audio_listener'
    ///   - 'signal_generator'
    ///   - 'wav_recorder'
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub full_stack_tools: bool,

    /// Include tools for legacy audio driver development, such as:
    ///   - 'audio-codec-ctl'
    ///   - 'audio-driver-ctl'
    ///   - 'dsputil'
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub legacy_driver_tools: bool,

    /// Configuration for virtual audio drivers and corresponding CLI utilities.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub virtual_audio: Option<VirtualAudioConfig>,
}

/// Configuration for virtual audio drivers (`virtual-audio` /
/// `virtual-audio-legacy`) and CLI utils.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct VirtualAudioConfig {
    /// Whether to include legacy virtual audio driver (`virtual-audio-legacy`).
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub legacy: bool,

    /// Whether to include modern virtual audio driver (`virtual-audio`).
    /// If omitted in JSON, this defaults to `!legacy` (i.e., true if legacy
    /// is false, false if legacy is true).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modern: Option<bool>,

    /// Whether to include the corresponding CLI utilities
    /// (`virtual_audio_util` and `virtual_audio_legacy_util`).
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub tools: bool,
}

impl AudioToolsConfig {
    /// Returns whether the modern virtual audio driver (`virtual-audio`) is enabled.
    pub fn is_virtual_audio_modern_enabled(&self) -> bool {
        self.virtual_audio.as_ref().is_some_and(|v| v.is_modern_enabled())
    }

    /// Returns whether the legacy virtual audio driver (`virtual-audio-legacy`) is enabled.
    pub fn is_virtual_audio_legacy_enabled(&self) -> bool {
        self.virtual_audio.as_ref().is_some_and(|v| v.legacy)
    }

    /// Returns whether the virtual audio CLI utilities (`virtual_audio_util` /
    /// `virtual_audio_legacy_util`) are enabled.
    pub fn is_virtual_audio_tools_enabled(&self) -> bool {
        self.virtual_audio.as_ref().is_some_and(|v| v.tools)
    }
}

impl VirtualAudioConfig {
    pub fn is_modern_enabled(&self) -> bool {
        self.modern.unwrap_or(!self.legacy)
    }
}

/// Platform-provided tools for development and debugging connectivity.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ConnectivityToolsConfig {
    /// Include tools for basic networking, such as:
    ///   - 'nc'
    ///   - 'iperf3'
    ///   - 'tcpdump'
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub enable_networking: bool,

    /// Include tools for wlan
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub enable_wlan: bool,

    /// Include tools for Thread
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub enable_thread: bool,
}

/// Platform-provided tools for the development and debugging of storage.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct StorageToolsConfig {
    /// Include tools used for disk partitioning, such as:
    ///   - 'mount'
    ///   - 'install-disk-image'
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub enable_partitioning_tools: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct HeapdumpConfig {
    /// Whether component manager will be instrumented.
    pub component_manager: bool,

    /// Whether driver_manager and driver_host will be instrumented.
    pub driver_framework: bool,

    /// Monikers that the ELF runner will inject the instrumentation into.
    pub monikers: Vec<String>,
}

#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TracingConfig {
    /// Tracing is disabled.
    #[default]
    Disabled,
    /// Tracing is enabled.
    Enabled {},
}

impl HeapdumpConfig {
    /// Tells if at least one program will be instrumented.
    pub fn is_enabled(&self) -> bool {
        self.component_manager || self.driver_framework || !self.monikers.is_empty()
    }
}

impl DevelopmentSupportConfig {
    /// Tells if at least one program will have tracing enabled.
    pub fn tracing_enabled(
        &self,
        feature_set_level: FeatureSetLevel,
        build_type: BuildType,
    ) -> Result<bool> {
        let option_enabled =
            self.include_tracing || matches!(self.tracing, TracingConfig::Enabled { .. });
        match (feature_set_level, build_type) {
            // Tracing is always enabled on standard eng.
            (FeatureSetLevel::Standard, BuildType::Eng) => Ok(true),

            // Tracing is enabled on userdebug or eng for other feature set levels
            // if the user explicitly requests it.
            (_, BuildType::UserDebug | BuildType::Eng) => Ok(option_enabled),

            // Tracing is never enabled on user.
            (_, BuildType::User) => {
                if option_enabled {
                    anyhow::bail!("tracing can't be included in user builds");
                }
                Ok(false)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_tools_virtual_audio_none() {
        let config = AudioToolsConfig::default();
        assert!(!config.is_virtual_audio_modern_enabled());
        assert!(!config.is_virtual_audio_legacy_enabled());
        assert!(!config.is_virtual_audio_tools_enabled());
    }

    #[test]
    fn test_audio_tools_virtual_audio_default_modern() {
        let config = AudioToolsConfig {
            virtual_audio: Some(VirtualAudioConfig { legacy: false, modern: None, tools: true }),
            ..Default::default()
        };
        assert!(config.is_virtual_audio_modern_enabled());
        assert!(!config.is_virtual_audio_legacy_enabled());
        assert!(config.is_virtual_audio_tools_enabled());
    }

    #[test]
    fn test_audio_tools_virtual_audio_legacy_only() {
        let config = AudioToolsConfig {
            virtual_audio: Some(VirtualAudioConfig { legacy: true, modern: None, tools: false }),
            ..Default::default()
        };
        assert!(!config.is_virtual_audio_modern_enabled());
        assert!(config.is_virtual_audio_legacy_enabled());
        assert!(!config.is_virtual_audio_tools_enabled());
    }

    #[test]
    fn test_audio_tools_virtual_audio_both() {
        let config = AudioToolsConfig {
            virtual_audio: Some(VirtualAudioConfig {
                legacy: true,
                modern: Some(true),
                tools: true,
            }),
            ..Default::default()
        };
        assert!(config.is_virtual_audio_modern_enabled());
        assert!(config.is_virtual_audio_legacy_enabled());
        assert!(config.is_virtual_audio_tools_enabled());
    }

    #[test]
    fn test_audio_tools_virtual_audio_explicit_disabled() {
        let config = AudioToolsConfig {
            virtual_audio: Some(VirtualAudioConfig {
                legacy: false,
                modern: Some(false),
                tools: false,
            }),
            ..Default::default()
        };
        assert!(!config.is_virtual_audio_modern_enabled());
        assert!(!config.is_virtual_audio_legacy_enabled());
        assert!(!config.is_virtual_audio_tools_enabled());
    }
}
