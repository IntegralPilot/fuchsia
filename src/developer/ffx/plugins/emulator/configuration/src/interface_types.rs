// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Common data types for emulator engines. These are implementation-agnostic
//! data types, not the engine-specific command types that each engine will define for itself. These
//! types will be directly deserializable from the PBM, and converted into engine-specific types at
//! runtime.

use crate::enumerations::{EngineConsoleType, ShowDetail};
use async_trait::async_trait;
use emulator_instance::{EmulatorConfiguration, EmulatorInstanceData, EngineState, EngineType};
use ffx_config::EnvironmentContext;
use fho::Result;
use std::backtrace::Backtrace;
use std::path::Path;
use std::process::Command;

#[async_trait(?Send)]
pub trait EmulatorEngine: Send + Sync {
    /// Expose the EmulatorInstanceData object. This stores all the instance data for the
    /// emulator instance. There is no "mut" getter to avoid changing the contents without
    /// coordinating via the concrete type.
    fn get_instance_data(&self) -> &EmulatorInstanceData {
        let bt = Backtrace::force_capture();
        unimplemented!(
            "These default trait implementations are to allow for easier testing and \
            should not be used directly\n{:?}",
            bt
        )
    }

    /// Prepare an emulator to run. This function shouldn't require any additional configuration as
    /// input, since the object should be fully configured by the EngineBuilder. At its most basic,
    /// this should assemble the command-line to invoke the emulator binary. If support processes
    /// are required, or temporary files need to be written to disk, that would be handled here.
    async fn stage(&mut self) -> Result<()> {
        let bt = Backtrace::force_capture();
        unimplemented!(
            "These default trait implementations are to allow for easier testing and \
            should not be used directly\n{:?}",
            bt
        )
    }

    /// Given a staged emulator instance, start it running. When the function returns, either the
    /// emulator will be running independently, or an error will be sent back explaining the failure.
    async fn start(
        &mut self,
        _context: &EnvironmentContext,
        mut _emulator_cmd: Command,
    ) -> Result<i32> {
        let bt = Backtrace::force_capture();
        unimplemented!(
            "These default trait implementations are to allow for easier testing and \
            should not be used directly\n{:?}",
            bt
        )
    }

    /// Shut down a running emulator instance. The engine should have been instantiated from a saved
    /// and serialized instance, so no additional initialization should be needed. This function
    /// will terminate a running emulator instance, which will be specified on the command line. It
    /// may return an error if the instance doesn't exist or the shut down fails, but should succeed
    /// if it's no longer running or gets successfully shut down.
    async fn stop(&mut self) -> Result<()> {
        let bt = Backtrace::force_capture();
        unimplemented!(
            "These default trait implementations are to allow for easier testing and \
            should not be used directly\n{:?}",
            bt
        )
    }

    /// Takes a screenshot of the emulator's primary display and saves it to the given path.
    ///
    /// On success, returns [`Ok(())`] and creates a valid PNG file at `path`.
    /// If a file already exists at `path`, it will be overwritten.
    ///
    /// The output is a PNG file in RGB color space; it has the same resolution
    /// as the emulator's primary display device.
    ///
    /// # Panics
    ///
    /// This method will panic if:
    /// * The emulator is not in the [`EngineState::Running`] state.
    /// * A virtual display is not enabled in the emulator configuration.
    /// * The output path is a directory.
    /// * The parent directory of `path` is not writable.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying capture, conversion, or file I/O fails.
    async fn screenshot(&mut self, _path: &Path) -> Result<()> {
        let bt = Backtrace::force_capture();
        unimplemented!(
            "These default trait implementations are to allow for easier testing and \
            should not be used directly\n{:?}",
            bt
        )
    }

    /// Output the details of an existing emulation instance. The engine should have been
    /// instantiated from a saved and serialized instance, so no additional initialization should be
    /// needed. This function will output text to the terminal describing the instance, its status,
    /// and its configuration. This is an engine-specific output with more detail than `ffx list`.
    fn show(&self, _details: Vec<ShowDetail>) -> Vec<ShowDetail> {
        let bt = Backtrace::force_capture();
        unimplemented!(
            "These default trait implementations are to allow for easier testing and \
            should not be used directly\n{:?}",
            bt
        )
    }

    /// Complete and validate the configuration parameters that have been provided to this engine,
    /// according to the requirements for this engine type. If there are fields which are required,
    /// mutually exclusive, only work when applied in certain combinations, or don't apply to this
    /// engine type, this function will return an error indicating which field(s) and why. It also
    /// returns an error if the engine has already been started. Otherwise, this returns Ok(()).
    fn configure(&mut self) -> Result<()> {
        let bt = Backtrace::force_capture();
        unimplemented!(
            "These default trait implementations are to allow for easier testing and \
            should not be used directly\n{:?}",
            bt
        )
    }

    /// Returns the EngineType used when building this engine. Each engine implementation should
    /// always return the same EngineType.
    fn engine_type(&self) -> EngineType {
        let bt = Backtrace::force_capture();
        unimplemented!(
            "These default trait implementations are to allow for easier testing and \
            should not be used directly\n{:?}",
            bt
        )
    }

    /// Returns true if this instance of the emulator is currently running.
    /// This is checked by using signal to the process id, no consideration is
    /// made for multi threaded access.
    async fn is_running(&mut self) -> bool {
        let bt = Backtrace::force_capture();
        unimplemented!(
            "These default trait implementations are to allow for easier testing and \
            should not be used directly\n{:?}",
            bt
        )
    }

    /// Once the engine has been staged, this generates the command line required to start
    /// emulation. There are no side-effects, and the operation can be repeated as necessary.
    fn build_emulator_cmd(&self) -> Command {
        let bt = Backtrace::force_capture();
        unimplemented!(
            "These default trait implementations are to allow for easier testing and \
            should not be used directly\n{:?}",
            bt
        )
    }

    /// Determine the appropriate binary for the target engine, and load its path into the
    /// engine's configuration for future use.
    fn load_emulator_binary(&mut self) -> Result<()> {
        let bt = Backtrace::force_capture();
        unimplemented!(
            "These default trait implementations are to allow for easier testing and \
            should not be used directly\n{:?}",
            bt
        )
    }

    /// Access to the engine's emulator_configuration field.
    fn emu_config(&self) -> &EmulatorConfiguration {
        let bt = Backtrace::force_capture();
        unimplemented!(
            "These default trait implementations are to allow for easier testing and \
            should not be used directly\n{:?}",
            bt
        )
    }

    /// Mutable access to the engine's emulator_configuration field.
    fn emu_config_mut(&mut self) -> &mut EmulatorConfiguration {
        let bt = Backtrace::force_capture();
        unimplemented!(
            "These default trait implementations are to allow for easier testing and \
            should not be used directly\n{:?}",
            bt
        )
    }

    /// Attach the current process to one of the emulator's consoles.
    fn attach(&self, _console: EngineConsoleType) -> Result<()> {
        let bt = Backtrace::force_capture();
        unimplemented!(
            "These default trait implementations are to allow for easier testing and \
            should not be used directly\n{:?}",
            bt
        )
    }

    /// Access to the engine's engine_state field.
    fn engine_state(&self) -> EngineState {
        let bt = Backtrace::force_capture();
        unimplemented!(
            "These default trait implementations are to allow for easier testing and \
            should not be used directly\n{:?}",
            bt
        )
    }
    /// Serialize the contents of the EmulatorEngine to its instance directory on the
    /// local file system.
    async fn save_to_disk(&self) -> Result<()> {
        let bt = Backtrace::force_capture();
        unimplemented!(
            "These default trait implementations are to allow for easier testing and \
            should not be used directly\n{:?}",
            bt
        )
    }
}
