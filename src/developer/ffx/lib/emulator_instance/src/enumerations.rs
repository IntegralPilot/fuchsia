// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module holds the common data types for emulator engines. These are implementation-agnostic
//! data types, not the engine-specific command types that each engine will define for itself. These
//! types will be directly deserializable from the PBM, and converted into engine-specific types at
//! runtime.

use schemars::JsonSchema;
pub use sdk_metadata::CpuArchitecture;
use sdk_metadata::display_impl;
use serde::{Deserialize, Serialize};

/// Selector for which type of hardware acceleration will be enabled for the emulator.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccelerationMode {
    /// The emulator will set the acceleration mode according to the host system's capabilities.
    Auto,

    /// KVM or similar acceleration will be enabled.
    Hyper,

    /// Hardware acceleration is disabled.
    None,
}

impl Default for AccelerationMode {
    fn default() -> Self {
        AccelerationMode::None
    }
}

display_impl!(AccelerationMode);

/// Selector for the launcher's output once the system is running.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleType {
    /// The launcher will finish by opening the Fuchsia serial console.
    Console,

    /// The launcher will finish by returning the user to the host's command prompt.
    None,
}

impl Default for ConsoleType {
    fn default() -> Self {
        ConsoleType::None
    }
}

display_impl!(ConsoleType);

/// The emulator engine follows a strict state transition graph, as outlined below.
///
///               ---------------------------------------
///               |             |                       v
/// New ---> Configured ---> Staged <---> Running ---> end
///  |            |             |            |          ^
///  ----------------> Error <----------------          |
///                      |                              |
///                      -------------------------------|
///
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum EngineState {
    /// The New state is the initial state of the engine. This is the state assigned to the
    /// emulator when the data structure is first initialized. It indicates nothing has
    /// occurred yet.
    New,

    /// When an emulator is in the Configured state, it indicates that the input has been processed
    /// and the EmulationConfiguration populated with the values provided by the user. The emulator
    /// instance directory has been created, but only the engine state and configuration are there.
    /// When the --dry-run flag is included, this is where the `start` subcommand terminates. There
    /// are no commands to trigger state transitions for an engine that has landed in the
    /// Configured state, but the configuration can be observed, saved, or modified for use on
    /// future instances with `ffx emu show --config` or similar subcommands.
    Configured,

    /// When an emulator is in the Staged state, it indicates that configuration step was
    /// successful and that the files needed to run the emulator have been copied into the instance
    /// directory, and any required modifications to those files have been performed. An emulator
    /// can also reach Staged without copying runtime files to the instance directory if the user
    /// provides a custom configuration with the --config flag; any files specified in such a
    /// configuration are unverified, unmodified, and executed from the location provided rather
    /// than staged.
    ///
    /// When the --stage flag is included, this is where the `start` subcommand terminates. This is
    /// also the state of an emulator that was running but has been stopped by some means, such as
    /// `ffx emu stop --persist`, or `power off` in the serial console. A Staged instance can be
    /// (re)started with `ffx emu start --reuse`.
    Staged,

    /// When an emulator is in the Running state, it indicates that there is a process on the host
    /// executing the Fuchsia system according to the engine's configuration. There are no
    /// guarantees about the guest regarding accessibility or progress on any internal boot
    /// processes.
    Running,

    /// An emulator in the Error state has no guarantees about the internal state. This state may
    /// be entered from any other state for any reason. An emulator in the Error state is no longer
    /// viable for execution or any other forward progress; it exists for retrospective analysis
    /// and debugging purposes only. All artifacts that were generated in previous states are
    /// retained in the instance directory for this purpose.
    Error,
    // When an emulator reaches the end state, it indicates that the instance has been terminated
    // and the instance directory has been removed from the filesystem. There is no enumeration
    // value for this state, because it can never be assigned or observed.
    //
    // The end state can be reached from any other state with the `ffx emu stop` command.
}

impl Default for EngineState {
    fn default() -> Self {
        EngineState::New
    }
}

display_impl!(EngineState);

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum EngineType {
    /// Fuchsia Emulator based on AEMU. Supports graphics.
    Femu,

    /// Qemu emulator.
    Qemu,

    /// Crosvm emulator.
    Crosvm,
}

impl Default for EngineType {
    fn default() -> Self {
        EngineType::Femu
    }
}

display_impl!(EngineType);

/// Selector for which type of graphics acceleration to enable for the emulator.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub enum GpuType {
    /// Let the emulator choose between hardware or software graphics
    /// acceleration based on your computer setup.
    ///
    /// For experimental use only; not officially supported by the Fuchsia
    /// emulator team.
    #[serde(rename = "auto")]
    AutoExperimental,

    /// Use the GPU on your computer for hardware acceleration. This option
    /// typically provides the highest graphics quality and performance for the
    /// emulator. However, if your graphics drivers have issues rendering
    /// OpenGL, you might need to use the swiftshader_indirect option.
    ///
    /// For experimental use only; not officially supported by the Fuchsia
    /// emulator team.
    #[serde(rename = "host")]
    HostExperimental,

    /// Use a Quick Boot-compatible variant of SwiftShader to render graphics
    /// using software acceleration.
    #[serde(rename = "swiftshader_indirect")]
    SwiftshaderIndirect,
}

impl Default for GpuType {
    fn default() -> Self {
        GpuType::SwiftshaderIndirect
    }
}

display_impl!(GpuType);

/// Selector for the verbosity level of the logs for this instance.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    /// Logs will contain entries indicating progress and some configuration details.
    Info,

    /// Logs will contain all entries currently generated by the system. Useful for debugging.
    Verbose,
}

impl Default for LogLevel {
    fn default() -> Self {
        LogLevel::Info
    }
}

display_impl!(LogLevel);

/// Indicates the Operating System the system is running.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OperatingSystem {
    Linux,
    MacOS,
    Unsupported,
}

impl Default for OperatingSystem {
    fn default() -> Self {
        OperatingSystem::Unsupported
    }
}

impl From<String> for OperatingSystem {
    fn from(item: String) -> Self {
        match &item[..] {
            // Values based on https://doc.rust-lang.org/std/env/consts/constant.OS.html
            "linux" => Self::Linux,
            "macos" => Self::MacOS,
            _ => Self::Unsupported,
        }
    }
}

display_impl!(OperatingSystem);

/// Selector for the mode of networking to enable between the guest and host systems.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NetworkingMode {
    /// Allow the system to determine the best available networking option. Typically
    /// this means "Tap", falling back to "User" if Tap is unavailable.
    Auto,

    /// Networking will be set up in bridged mode, using an interface such as tun/tap.
    Tap,

    /// Networking will be over explicitly mapped ports, using an interface such as SLiRP.
    User,

    /// Guest networking will be disabled.
    None,
}

impl Default for NetworkingMode {
    fn default() -> Self {
        NetworkingMode::Auto
    }
}

display_impl!(NetworkingMode);

/// Definition of the CPU type(s) and how many of them to emulate.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct VirtualCpu {
    /// The guest system's CPU architecture, i.e. the CPU type that will be emulated in the guest.
    pub architecture: CpuArchitecture,

    /// The number of virtual CPUs that will emulated in the virtual device.
    pub count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Result;
    use std::str::FromStr;

    #[test]
    fn test_accel() -> Result<()> {
        // Verify it returns a default.
        let _default = AccelerationMode::default();
        // Deserialize a valid value.
        assert!(AccelerationMode::from_str("auto").is_ok());
        // Fail to deserialize an invalid value.
        assert!(AccelerationMode::from_str("bad_value").is_err());
        Ok(())
    }

    #[test]
    fn test_console() -> Result<()> {
        // Verify it returns a default.
        let _default = ConsoleType::default();
        Ok(())
    }

    #[test]
    fn test_engine() -> Result<()> {
        // Verify it returns a default.
        let _default = EngineType::default();
        // Deserialize a valid value.
        assert!(EngineType::from_str("qemu").is_ok());
        // Fail to deserialize an invalid value.
        assert!(EngineType::from_str("bad_value").is_err());
        Ok(())
    }

    #[test]
    fn test_gpu() -> Result<()> {
        // Verify it returns a default
        let default = GpuType::default();
        // Verify we can use default formatting to print it.
        println!("{}", default);
        // Deserialize a valid value.
        assert!(GpuType::from_str("auto").is_ok());
        // Fail to deserialize an invalid value.
        assert!(GpuType::from_str("bad_value").is_err());
        Ok(())
    }

    #[test]
    fn test_log() -> Result<()> {
        // Verify it returns a default.
        let _default = LogLevel::default();
        Ok(())
    }

    #[test]
    fn test_net() -> Result<()> {
        // Verify it returns a default.
        let _default = NetworkingMode::default();
        Ok(())
    }

    #[test]
    fn test_cpu() -> Result<()> {
        // Verify it returns a default.
        let _default = VirtualCpu::default();
        Ok(())
    }
}
