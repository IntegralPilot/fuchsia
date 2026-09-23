// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod fastboot_interface;
pub mod fastboot_proxy;
pub mod interface_factory;
pub mod stream;
pub mod util;

pub mod test {
    use super::fastboot_interface::{
        Fastboot, FastbootError, FastbootInterface, RebootEvent, UploadProgress, Variable,
    };
    use super::stream::StreamCommand;
    use async_trait::async_trait;
    use chrono::Duration;
    use std::collections::HashMap;
    use std::default::Default;
    use std::sync::{Arc, Mutex};
    use tokio::sync::mpsc::Sender;

    #[derive(Default, Debug)]
    pub struct FakeServiceCommands {
        pub staged_files: Vec<String>,
        /// All transmitted OEM commands; each will have the "oem " prefix automatically inserted
        /// to match real fastboot behavior.
        pub oem_commands: Vec<String>,
        pub bootloader_reboots: usize,
        pub boots: usize,
        pub continue_boots: usize,
        pub set_actives: Vec<String>,
        /// Variable => (Option(Value), Call Count)
        variables: HashMap<String, (Option<String>, u32)>,
    }

    impl FakeServiceCommands {
        /// Sets the provided variable to the given value preserving the past
        /// call count.
        pub fn set_var(&mut self, var: String, value: String) {
            match self.variables.get_mut(&var) {
                Some((old_val, _)) => *old_val = Some(value),
                None => {
                    self.variables.insert(var, (Some(value), 0));
                }
            }
        }

        /// Returns (variable_set, call_count)
        pub fn get_var_call_count(&self, var: impl Into<String>) -> (bool, u32) {
            self.variables
                .get(&var.into())
                .map(|(val, count)| (val.is_some(), *count))
                .unwrap_or((false, 0))
        }

        /// Helper for setting multiple variables at once
        pub fn set_multiple_vars(
            &mut self,
            vars: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
        ) {
            vars.into_iter().for_each(|(var, val)| self.set_var(var.into(), val.into()))
        }
    }

    #[derive(Debug)]
    pub struct TestFastbootInterface {
        state: Arc<Mutex<FakeServiceCommands>>,
    }

    impl TestFastbootInterface {
        pub fn new(state: Arc<Mutex<FakeServiceCommands>>) -> Self {
            Self { state }
        }
    }

    impl FastbootInterface for TestFastbootInterface {}

    #[async_trait]
    impl Fastboot for TestFastbootInterface {
        async fn get_var(&mut self, name: &str) -> Result<String, FastbootError> {
            let mut state = self.state.lock().unwrap();
            match state.variables.get_mut(name) {
                Some((Some(val), count)) => {
                    *count += 1;
                    Ok(val.clone())
                }
                Some((None, count)) => {
                    *count += 1;
                    Err(FastbootError::GetVariableError {
                        variable: name.to_string(),
                        message: "Variable not found".to_string(),
                    })
                }
                None => {
                    state.variables.insert(name.to_string(), (None, 1));
                    Err(FastbootError::GetVariableError {
                        variable: name.to_string(),
                        message: "Variable not found".to_string(),
                    })
                }
            }
        }

        async fn get_all_vars(&mut self, listener: Sender<Variable>) -> Result<(), FastbootError> {
            listener
                .send(Variable { name: "test".to_string(), value: "test".to_string() })
                .await
                .unwrap();
            Ok(())
        }

        async fn flash(
            &mut self,
            _partition_name: &str,
            _path: &str,
            listener: Sender<UploadProgress>,
            _timeout: Duration,
        ) -> Result<(), FastbootError> {
            listener
                .send(UploadProgress::OnStarted { size: 1 })
                .await
                .map_err(|_| FastbootError::ProgressSendError)?;
            listener
                .send(UploadProgress::OnProgress { bytes_written: 1 })
                .await
                .map_err(|_| FastbootError::ProgressSendError)?;
            listener
                .send(UploadProgress::OnFinished)
                .await
                .map_err(|_| FastbootError::ProgressSendError)?;
            Ok(())
        }

        async fn flash_from_reader(
            &mut self,
            _partition_name: &str,
            size: u32,
            _reader: &mut (dyn std::io::Read + Send),
            listener: Sender<UploadProgress>,
            _timeout: Duration,
        ) -> Result<(), FastbootError> {
            listener
                .send(UploadProgress::OnStarted { size: size as u64 })
                .await
                .map_err(|_| FastbootError::ProgressSendError)?;
            listener
                .send(UploadProgress::OnProgress { bytes_written: size as u64 })
                .await
                .map_err(|_| FastbootError::ProgressSendError)?;
            listener
                .send(UploadProgress::OnFinished)
                .await
                .map_err(|_| FastbootError::ProgressSendError)?;
            Ok(())
        }

        async fn erase(&mut self, _partition_name: &str) -> Result<(), FastbootError> {
            Ok(())
        }

        async fn boot(&mut self) -> Result<(), FastbootError> {
            let mut state = self.state.lock().unwrap();
            state.boots += 1;
            Ok(())
        }

        async fn reboot(&mut self) -> Result<(), FastbootError> {
            Ok(())
        }

        async fn reboot_bootloader(
            &mut self,
            listener: Sender<RebootEvent>,
        ) -> Result<(), FastbootError> {
            listener.send(RebootEvent::OnReboot).await.unwrap();
            let mut state = self.state.lock().unwrap();
            state.bootloader_reboots += 1;
            Ok(())
        }

        async fn continue_boot(&mut self) -> Result<(), FastbootError> {
            let mut state = self.state.lock().unwrap();
            state.continue_boots += 1;
            Ok(())
        }

        async fn get_staged(&mut self, _path: &str) -> Result<(), FastbootError> {
            Ok(())
        }

        async fn stage(
            &mut self,
            path: &str,
            _listener: Sender<UploadProgress>,
        ) -> Result<(), FastbootError> {
            let mut state = self.state.lock().unwrap();
            state.staged_files.push(path.to_string());
            Ok(())
        }

        async fn set_active(&mut self, slot: &str) -> Result<(), FastbootError> {
            let mut state = self.state.lock().unwrap();
            state.set_actives.push(slot.to_string());
            Ok(())
        }

        async fn oem(&mut self, command: &str) -> Result<(), FastbootError> {
            let mut state = self.state.lock().unwrap();
            state.oem_commands.push(format!("oem {}", command));
            Ok(())
        }

        async fn stream<'a>(
            &mut self,
            _partition_name: &str,
            _stream_command: StreamCommand,
            listener: &Sender<UploadProgress>,
            _timeout: Duration,
        ) -> Result<(), FastbootError> {
            listener
                .send(UploadProgress::OnProgress { bytes_written: 1 })
                .await
                .map_err(|_| FastbootError::ProgressSendError)
        }
    }

    pub fn setup() -> (Arc<Mutex<FakeServiceCommands>>, TestFastbootInterface) {
        let state = Arc::new(Mutex::new(FakeServiceCommands::default()));
        let interface = TestFastbootInterface { state: state.clone() };
        (state, interface)
    }
}
