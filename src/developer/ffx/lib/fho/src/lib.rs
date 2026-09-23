// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod adapters;
mod fho_env;
mod from_env;
mod try_from_env;

pub mod null_writer;
pub mod subtool;
pub mod subtool_suite;

pub use subtool::{FfxMain, FfxTool};
pub use subtool_suite::{Subtool, SubtoolBox};

// Re-export TryFromEnv related symbols
pub use fho_env::{EnvironmentInterface, FhoEnvironment};
pub use from_env::{AvailabilityFlag, CheckEnv};

pub use try_from_env::{Deferred, TryFromEnv, TryFromEnvWith, deferred};

// Used for deriving an FFX tool.
pub use fho_macro::{FfxError, FfxTool};

// Re-expose the Error, Result, and FfxContext types from ffx_command
// so you don't have to pull both in all the time.
pub use ffx_command_error::{
    Error, FfxContext, NonFatalError, Result, bug, exit_with_code, return_bug, return_user_error,
    user_error,
};

// FfxCommandLine is being re-exported so that, it can easily be used by the derive macros for
// subtools.
pub use ffx_command::FfxCommandLine;

#[doc(hidden)]
pub mod macro_deps {
    pub use crate as fho;
    pub use anyhow;
    pub use argh;
    pub use async_lock;
    pub use async_trait::async_trait;
    pub use ffx_command::{
        Ffx, ToolRunner, bug, check_strict_constraints, return_bug, return_user_error,
    };
    pub use ffx_config::EnvironmentContext;
    pub use ffx_diagnostics_analytics_state;
    pub use futures;
    pub use serde;
    pub use writer;
}
