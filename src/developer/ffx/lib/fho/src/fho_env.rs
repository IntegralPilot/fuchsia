// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use ffx_command::FfxCommandLine;
use ffx_command_error::{Result, return_bug};
use ffx_config::EnvironmentContext;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

// Interfaces can provide additional errors
pub trait EnvironmentInterface: Any {
    fn wrap_main_errors(&self, err: crate::Error) -> crate::Error;
}

// Type magic allowing us to both cast to Any, as well as to get back to this interface
impl dyn EnvironmentInterface {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone, Default)]
pub struct FhoEnvironment {
    ffx: FfxCommandLine,
    context: EnvironmentContext,
    // Store information relevant to dependent crates, at most one for each type.
    interfaces: Arc<Mutex<HashMap<TypeId, Box<dyn EnvironmentInterface>>>>,
}

impl FhoEnvironment {
    pub fn new(context: &EnvironmentContext, ffx: &FfxCommandLine) -> Self {
        log::info!("FhoEnvironment created");
        let ffx = ffx.clone();
        FhoEnvironment { ffx, context: context.clone(), interfaces: Default::default() }
    }

    /// Create new instance for use in tests.
    pub fn new_with_args(context: &EnvironmentContext, argv: &[impl AsRef<str>]) -> Self {
        log::info!("FhoEnvironment test instance with args created");
        let ffx = FfxCommandLine::new(None, argv).unwrap();
        FhoEnvironment { ffx, context: context.clone(), interfaces: Default::default() }
    }

    pub fn ffx_command(&self) -> &FfxCommandLine {
        &self.ffx
    }

    pub fn environment_context(&self) -> &EnvironmentContext {
        &self.context
    }

    /// Update the log file name which can be influenced by the
    /// FfxMain implementation being run.
    pub fn update_log_file(&self, basename: Option<String>) -> Result<()> {
        if let Some(basename) = basename {
            // If the base name is the default, no action is needed.
            if basename == ffx_config::logging::LOG_BASENAME {
                return Ok(());
            }
            // If the log was specified on the command line, no action is needed
            if self.ffx.global.log_destination.is_some() {
                return Ok(());
            }
            // Some simple validation of the basename.
            if basename.is_empty() {
                return_bug!("basename cannot be empty")
            }
            // Build the path to the new log file.
            let dir: PathBuf =
                self.context.get(ffx_config::logging::LOG_DIR).unwrap_or_else(|_| ".".into());
            let mut log_file = dir.join(basename);
            log_file.set_extension("log");
            log::info!("Switching log file to {log_file:?}");
            eprintln!("Switching log file to {log_file:?}");
            ffx_config::logging::change_log_file(&log_file).map_err(anyhow::Error::from)?;
        }
        Ok(())
    }

    pub fn get_interface<T: EnvironmentInterface + Clone + 'static>(&self) -> Option<T> {
        let interfaces = self.interfaces.lock().expect("poisoned interface map");
        let Some(ei) = interfaces.get(&TypeId::of::<T>()) else {
            return None;
        };
        ei.as_any().downcast_ref::<T>().map(|t| t.clone())
    }
    pub fn set_interface<T: EnvironmentInterface + 'static>(&self, ei: T) {
        let mut interfaces = self.interfaces.lock().expect("poisoned interface map");
        interfaces.insert(TypeId::of::<T>(), Box::new(ei));
    }

    pub fn wrap_main_result<T>(&self, res: Result<T>) -> Result<T> {
        match res {
            Ok(_) => res,
            Err(mut e) => {
                let interfaces = self.interfaces.lock().expect("poisoned interface map");
                for i in interfaces.values() {
                    e = i.wrap_main_errors(e);
                }
                Err(e)
            }
        }
    }
}
