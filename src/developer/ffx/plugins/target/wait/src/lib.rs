// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use errors;
use ffx_config::EnvironmentContext;
use ffx_diagnostics::Notifier;
use ffx_wait_args::{TargetStateOption, WaitOptions};
use ffx_writer::VerifiedMachineWriter;
use fho::{Error, FfxMain, FfxTool};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::time::Duration;

const DEFAULT_DIAGNOSTICS_TIMEOUT_SECS: f64 = 2.0;

#[derive(Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    /// Successfully waited for the target (either to come up or shut down).
    Ok {},
}

#[cfg_attr(test, mockall::automock)]
pub trait DeviceWaiter {
    fn wait(
        &self,
        dur: Option<Duration>,
        env: &EnvironmentContext,
        target_spec: &Option<String>,
        behavior: ffx_target::WaitFor,
    ) -> impl Future<Output = Result<(), fho::Error>>;
}

pub struct DeviceWaiterImpl;

#[async_trait(?Send)]
impl fho::TryFromEnv for DeviceWaiterImpl {
    type Error = std::convert::Infallible;
    async fn try_from_env(_env: &fho::FhoEnvironment) -> Result<Self, Self::Error> {
        Ok(DeviceWaiterImpl)
    }
}

impl DeviceWaiter for DeviceWaiterImpl {
    async fn wait(
        &self,
        dur: Option<Duration>,
        env: &EnvironmentContext,
        target_spec: &Option<String>,
        behavior: ffx_target::WaitFor,
    ) -> Result<(), fho::Error> {
        ffx_target::wait_for_device(dur, env, target_spec, behavior).await
    }
}

use fho::FfxError;
use thiserror::Error;

#[derive(FfxError, Error, Debug)]
pub enum WaitError {
    #[exit_with_code(1)]
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[exit_with_code(1)]
    #[error("Config error: {0}")]
    Config(#[from] ffx_config::api::ConfigError),

    #[exit_with_code(1)]
    #[error("FFX Writer error: {0}")]
    Writer(#[from] ffx_writer::Error),

    #[exit_with_code(1)]
    #[error("Wait operation failed:\n{0}")]
    WaitFailed(String),

    #[exit_with_code(1)]
    #[error("Failed waiting for target to shut down: {0}")]
    WaitDownFailed(fho::Error),

    #[exit_with_code(1)]
    #[error("{0}")]
    InvalidArgument(String),
}

#[derive(FfxTool)]
#[main_error(WaitError)]
pub struct WaitOperation<T: DeviceWaiter + fho::TryFromEnv> {
    #[command]
    pub cmd: WaitOptions,
    pub env: EnvironmentContext,
    pub waiter: T,
}

fho::embedded_plugin!(WaitOperation<DeviceWaiterImpl>, WaitError);

async fn get_diagnostics_string(env: &EnvironmentContext, timeout: u64, e: Error) -> String {
    let message = e.to_string();
    let timeout = if timeout > 0 {
        Duration::from_secs(timeout)
    } else {
        Duration::from_secs_f64(DEFAULT_DIAGNOSTICS_TIMEOUT_SECS)
    };
    let err = run_diagnostics(&env, timeout).await;
    format!("{message}\nDiagnostics:{err}")
}

async fn run_diagnostics(env: &EnvironmentContext, timeout: Duration) -> String {
    let mut notifier = ffx_diagnostics::StringNotifier::new();
    if let Err(e) = ffx_diagnostics_checks::run_diagnostics(&env, &mut notifier, timeout).await {
        notifier.on_error(format!("{e}")).unwrap();
    }
    notifier.into()
}

#[async_trait(?Send)]
impl<T: DeviceWaiter + fho::TryFromEnv> FfxMain for WaitOperation<T> {
    type Writer = VerifiedMachineWriter<CommandStatus>;
    type Error = WaitError;

    async fn main(self, mut writer: Self::Writer) -> Result<(), Self::Error> {
        let state = self.cmd.get_target_state().map_err(WaitError::InvalidArgument)?;
        match self.wait_impl(state).await {
            Ok(()) => {
                writer.machine(&CommandStatus::Ok {})?;
                Ok(())
            }
            Err(e) => match state {
                TargetStateOption::Down => {
                    // If we are waiting for the device to go down, a failure means we cannot confirm it is down.
                    // Running diagnostics makes no sense in this case.
                    Err(WaitError::WaitDownFailed(e))
                }
                TargetStateOption::Fastboot => {
                    // When waiting for fastboot, RCS is not running, so running host diagnostics makes no sense.
                    Err(WaitError::WaitFailed(e.to_string()))
                }
                TargetStateOption::Up | TargetStateOption::Product => {
                    let message = get_diagnostics_string(&self.env, self.cmd.timeout, e).await;
                    Err(WaitError::WaitFailed(message))
                }
            },
        }
    }
}

impl<T: DeviceWaiter + fho::TryFromEnv> WaitOperation<T> {
    pub async fn wait_impl(&self, state: TargetStateOption) -> Result<(), fho::Error> {
        let target_spec: Option<String> = ffx_target::get_target_specifier(&self.env)?;
        let behavior = match state {
            TargetStateOption::Down => ffx_target::WaitFor::DeviceOffline,
            TargetStateOption::Up => ffx_target::WaitFor::DeviceOnline,
            TargetStateOption::Fastboot => ffx_target::WaitFor::Fastboot,
            TargetStateOption::Product => ffx_target::WaitFor::Product,
        };
        let duration =
            if self.cmd.timeout > 0 { Some(Duration::from_secs(self.cmd.timeout)) } else { None };
        self.waiter.wait(duration, &self.env, &target_spec, behavior).await
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use argh::FromArgs;
    use ffx_writer::{Format, TestBuffers};
    use std::str::FromStr;

    // This is just here to satisfy trait bounds.
    #[async_trait(?Send)]
    impl fho::TryFromEnv for MockDeviceWaiter {
        type Error = std::convert::Infallible;
        async fn try_from_env(_env: &fho::FhoEnvironment) -> Result<Self, Self::Error> {
            unimplemented!()
        }
    }

    #[fuchsia::test]
    async fn test_args_parsing_defaults() {
        let options = WaitOptions::from_args(&["wait"], &[]).expect("parse default options");
        assert_eq!(options.timeout, 120);
        assert!(!options.down);
        assert_eq!(options.state, None);
        assert_eq!(options.get_target_state(), Ok(TargetStateOption::Up));
    }

    #[fuchsia::test]
    async fn test_args_parsing_down_flag() {
        let options = WaitOptions::from_args(&["wait"], &["--down"]).expect("parse --down option");
        assert_eq!(options.timeout, 120);
        assert!(options.down);
        assert_eq!(options.state, None);
        assert_eq!(options.get_target_state(), Ok(TargetStateOption::Down));

        let options_short = WaitOptions::from_args(&["wait"], &["-d"]).expect("parse -d option");
        assert!(options_short.down);
        assert_eq!(options_short.get_target_state(), Ok(TargetStateOption::Down));
    }

    #[fuchsia::test]
    async fn test_args_parsing_state_options() {
        let test_cases = [
            ("down", TargetStateOption::Down),
            ("up", TargetStateOption::Up),
            ("fastboot", TargetStateOption::Fastboot),
            ("product", TargetStateOption::Product),
            ("DOWN", TargetStateOption::Down),
            ("UP", TargetStateOption::Up),
            ("Fastboot", TargetStateOption::Fastboot),
            ("fastBOOT", TargetStateOption::Fastboot),
            ("PRODUCT", TargetStateOption::Product),
            ("Product", TargetStateOption::Product),
        ];

        for (input, expected) in test_cases {
            let options = WaitOptions::from_args(&["wait"], &["--state", input])
                .unwrap_or_else(|_| panic!("failed to parse state '{input}'"));
            assert_eq!(options.state, Some(expected));
            assert_eq!(options.get_target_state(), Ok(expected));
        }
    }

    #[fuchsia::test]
    async fn test_args_parsing_invalid_state() {
        assert!(TargetStateOption::from_str("invalid").is_err());
        assert!(WaitOptions::from_args(&["wait"], &["--state", "unknown"]).is_err());
    }

    #[fuchsia::test]
    async fn test_args_mutual_exclusivity() {
        for state in [
            TargetStateOption::Down,
            TargetStateOption::Up,
            TargetStateOption::Fastboot,
            TargetStateOption::Product,
        ] {
            let options = WaitOptions { timeout: 100, down: true, state: Some(state) };
            assert!(options.get_target_state().is_err());
        }
    }

    #[fuchsia::test]
    async fn test_mutual_exclusivity_main_error() {
        let test_env = ffx_config::test_init().expect("test env");
        let mock_waiter = MockDeviceWaiter::new();
        // Waiter should not even be called when arguments are invalid.
        let tool = WaitOperation {
            cmd: WaitOptions { timeout: 1000, down: true, state: Some(TargetStateOption::Up) },
            env: test_env.context.clone(),
            waiter: mock_waiter,
        };
        let test_buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<CommandStatus>::new_test(
            Some(Format::JsonPretty),
            &test_buffers,
        );
        let res = tool.main(writer).await;
        assert!(res.is_err());
        assert!(matches!(res, Err(WaitError::InvalidArgument(_))));
    }

    #[fuchsia::test]
    async fn test_success_default_up() {
        let test_env = ffx_config::test_init().expect("test env");
        let mut mock_waiter = MockDeviceWaiter::new();
        mock_waiter
            .expect_wait()
            .withf(|dur, _, _, behavior| {
                *dur == Some(Duration::from_secs(1000))
                    && *behavior == ffx_target::WaitFor::DeviceOnline
            })
            .times(1)
            .returning(|_, _, _, _| Box::pin(async { Ok(()) }));
        let tool = WaitOperation {
            cmd: WaitOptions { timeout: 1000, down: false, state: None },
            env: test_env.context.clone(),
            waiter: mock_waiter,
        };
        let test_buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<CommandStatus>::new_test(
            Some(Format::JsonPretty),
            &test_buffers,
        );
        let res = tool.main(writer).await;
        let (stdout, stderr) = test_buffers.into_strings();
        assert!(res.is_ok(), "expected ok {stdout} {stderr}");
        let err = format!("schema not valid {stdout}");
        let json = serde_json::from_str(&stdout).expect(&err);
        let err = format!("json must adhere to schema: {json}");
        VerifiedMachineWriter::<CommandStatus>::verify_schema(&json).expect(&err);
    }

    #[fuchsia::test]
    async fn test_success_state_up() {
        let test_env = ffx_config::test_init().expect("test env");
        let mut mock_waiter = MockDeviceWaiter::new();
        mock_waiter
            .expect_wait()
            .withf(|dur, _, _, behavior| {
                *dur == Some(Duration::from_secs(50))
                    && *behavior == ffx_target::WaitFor::DeviceOnline
            })
            .times(1)
            .returning(|_, _, _, _| Box::pin(async { Ok(()) }));
        let tool = WaitOperation {
            cmd: WaitOptions { timeout: 50, down: false, state: Some(TargetStateOption::Up) },
            env: test_env.context.clone(),
            waiter: mock_waiter,
        };
        let test_buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<CommandStatus>::new_test(
            Some(Format::JsonPretty),
            &test_buffers,
        );
        let res = tool.main(writer).await;
        assert!(res.is_ok());
    }

    #[fuchsia::test]
    async fn test_success_down_flag() {
        let test_env = ffx_config::test_init().expect("test env");
        let mut mock_waiter = MockDeviceWaiter::new();
        mock_waiter
            .expect_wait()
            .withf(|dur, _, _, behavior| {
                *dur == Some(Duration::from_secs(100))
                    && *behavior == ffx_target::WaitFor::DeviceOffline
            })
            .times(1)
            .returning(|_, _, _, _| Box::pin(async { Ok(()) }));
        let tool = WaitOperation {
            cmd: WaitOptions { timeout: 100, down: true, state: None },
            env: test_env.context.clone(),
            waiter: mock_waiter,
        };
        let test_buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<CommandStatus>::new_test(
            Some(Format::JsonPretty),
            &test_buffers,
        );
        let res = tool.main(writer).await;
        assert!(res.is_ok());
    }

    #[fuchsia::test]
    async fn test_success_state_down() {
        let test_env = ffx_config::test_init().expect("test env");
        let mut mock_waiter = MockDeviceWaiter::new();
        mock_waiter
            .expect_wait()
            .withf(|dur, _, _, behavior| {
                *dur == Some(Duration::from_secs(100))
                    && *behavior == ffx_target::WaitFor::DeviceOffline
            })
            .times(1)
            .returning(|_, _, _, _| Box::pin(async { Ok(()) }));
        let tool = WaitOperation {
            cmd: WaitOptions { timeout: 100, down: false, state: Some(TargetStateOption::Down) },
            env: test_env.context.clone(),
            waiter: mock_waiter,
        };
        let test_buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<CommandStatus>::new_test(
            Some(Format::JsonPretty),
            &test_buffers,
        );
        let res = tool.main(writer).await;
        assert!(res.is_ok());
    }

    #[fuchsia::test]
    async fn test_success_state_fastboot() {
        let test_env = ffx_config::test_init().expect("test env");
        let mut mock_waiter = MockDeviceWaiter::new();
        mock_waiter
            .expect_wait()
            .withf(|dur, _, _, behavior| {
                *dur == Some(Duration::from_secs(60)) && *behavior == ffx_target::WaitFor::Fastboot
            })
            .times(1)
            .returning(|_, _, _, _| Box::pin(async { Ok(()) }));
        let tool = WaitOperation {
            cmd: WaitOptions { timeout: 60, down: false, state: Some(TargetStateOption::Fastboot) },
            env: test_env.context.clone(),
            waiter: mock_waiter,
        };
        let test_buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<CommandStatus>::new_test(
            Some(Format::JsonPretty),
            &test_buffers,
        );
        let res = tool.main(writer).await;
        assert!(res.is_ok());
    }

    #[fuchsia::test]
    async fn test_success_state_product() {
        let test_env = ffx_config::test_init().expect("test env");
        let mut mock_waiter = MockDeviceWaiter::new();
        mock_waiter
            .expect_wait()
            .withf(|dur, _, _, behavior| {
                *dur == Some(Duration::from_secs(60)) && *behavior == ffx_target::WaitFor::Product
            })
            .times(1)
            .returning(|_, _, _, _| Box::pin(async { Ok(()) }));
        let tool = WaitOperation {
            cmd: WaitOptions { timeout: 60, down: false, state: Some(TargetStateOption::Product) },
            env: test_env.context.clone(),
            waiter: mock_waiter,
        };
        let test_buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<CommandStatus>::new_test(
            Some(Format::JsonPretty),
            &test_buffers,
        );
        let res = tool.main(writer).await;
        assert!(res.is_ok());
    }

    #[fuchsia::test]
    async fn test_success_state_timeout_zero() {
        let test_env = ffx_config::test_init().expect("test env");
        let mut mock_waiter = MockDeviceWaiter::new();
        mock_waiter
            .expect_wait()
            .withf(|dur, _, _, behavior| {
                dur.is_none() && *behavior == ffx_target::WaitFor::Fastboot
            })
            .times(1)
            .returning(|_, _, _, _| Box::pin(async { Ok(()) }));
        let tool = WaitOperation {
            cmd: WaitOptions { timeout: 0, down: false, state: Some(TargetStateOption::Fastboot) },
            env: test_env.context.clone(),
            waiter: mock_waiter,
        };
        let test_buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<CommandStatus>::new_test(
            Some(Format::JsonPretty),
            &test_buffers,
        );
        let res = tool.main(writer).await;
        assert!(res.is_ok());
    }

    #[fuchsia::test]
    async fn test_unexpected_error() {
        let test_env = ffx_config::test_init().expect("test env");
        let mut mock_waiter = MockDeviceWaiter::new();
        mock_waiter
            .expect_wait()
            .times(1)
            .returning(|_, _, _, _| Box::pin(async { Err(fho::bug!("oh no!")) }));
        let tool = WaitOperation {
            cmd: WaitOptions { timeout: 1000, down: false, state: None },
            env: test_env.context.clone(),
            waiter: mock_waiter,
        };
        let test_buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<CommandStatus>::new_test(
            Some(Format::JsonPretty),
            &test_buffers,
        );
        let res = tool.main(writer).await;
        let (stdout, stderr) = test_buffers.into_strings();
        assert!(res.is_err(), "expected error {stdout} {stderr}");
        assert!(
            matches!(res, Err(WaitError::WaitFailed(_))),
            "expected 'WaitFailed' error {stdout} {stderr}"
        );
    }

    #[fuchsia::test]
    async fn test_user_error() {
        let test_env = ffx_config::test_init().expect("test env");
        let mut mock_waiter = MockDeviceWaiter::new();
        mock_waiter
            .expect_wait()
            .times(1)
            .returning(|_, _, _, _| Box::pin(async { Err(fho::user_error!("oh no!")) }));
        let tool = WaitOperation {
            cmd: WaitOptions { timeout: 1000, down: false, state: None },
            env: test_env.context.clone(),
            waiter: mock_waiter,
        };
        let test_buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<CommandStatus>::new_test(
            Some(Format::JsonPretty),
            &test_buffers,
        );
        let res = tool.main(writer).await;
        let (stdout, stderr) = test_buffers.into_strings();
        assert!(res.is_err(), "expected error {stdout} {stderr}");
        assert!(
            matches!(res, Err(WaitError::WaitFailed(_))),
            "expected 'WaitFailed' error {stdout} {stderr}"
        );
    }

    #[fuchsia::test]
    async fn test_down_error_no_diagnostics() {
        let test_env = ffx_config::test_init().expect("test env");
        let mut mock_waiter = MockDeviceWaiter::new();
        mock_waiter
            .expect_wait()
            .times(1)
            .returning(|_, _, _, _| Box::pin(async { Err(fho::bug!("oh no!")) }));
        let tool = WaitOperation {
            cmd: WaitOptions { timeout: 1000, down: true, state: None },
            env: test_env.context.clone(),
            waiter: mock_waiter,
        };
        let test_buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<CommandStatus>::new_test(
            Some(Format::JsonPretty),
            &test_buffers,
        );
        let res = tool.main(writer).await;
        let (stdout, stderr) = test_buffers.into_strings();
        assert!(res.is_err(), "expected error {stdout} {stderr}");
        if let Err(WaitError::WaitDownFailed(e)) = res {
            let err_msg = e.to_string();
            assert!(err_msg.contains("oh no!"), "expected 'oh no!' in error message: {err_msg}");
            assert!(
                !err_msg.contains("Diagnostics:"),
                "did not expect 'Diagnostics:' in error message: {err_msg}"
            );
        } else {
            panic!("expected WaitDownFailed, got: {:?}", res);
        }
    }

    #[fuchsia::test]
    async fn test_state_down_error_no_diagnostics() {
        let test_env = ffx_config::test_init().expect("test env");
        let mut mock_waiter = MockDeviceWaiter::new();
        mock_waiter
            .expect_wait()
            .times(1)
            .returning(|_, _, _, _| Box::pin(async { Err(fho::bug!("oh no!")) }));
        let tool = WaitOperation {
            cmd: WaitOptions { timeout: 1000, down: false, state: Some(TargetStateOption::Down) },
            env: test_env.context.clone(),
            waiter: mock_waiter,
        };
        let test_buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<CommandStatus>::new_test(
            Some(Format::JsonPretty),
            &test_buffers,
        );
        let res = tool.main(writer).await;
        let (stdout, stderr) = test_buffers.into_strings();
        assert!(res.is_err(), "expected error {stdout} {stderr}");
        if let Err(WaitError::WaitDownFailed(e)) = res {
            let err_msg = e.to_string();
            assert!(err_msg.contains("oh no!"), "expected 'oh no!' in error message: {err_msg}");
            assert!(
                !err_msg.contains("Diagnostics:"),
                "did not expect 'Diagnostics:' in error message: {err_msg}"
            );
        } else {
            panic!("expected WaitDownFailed, got: {:?}", res);
        }
    }

    #[fuchsia::test]
    async fn test_fastboot_error_no_diagnostics() {
        let test_env = ffx_config::test_init().expect("test env");
        let mut mock_waiter = MockDeviceWaiter::new();
        mock_waiter
            .expect_wait()
            .times(1)
            .returning(|_, _, _, _| Box::pin(async { Err(fho::bug!("fastboot timeout")) }));
        let tool = WaitOperation {
            cmd: WaitOptions {
                timeout: 1000,
                down: false,
                state: Some(TargetStateOption::Fastboot),
            },
            env: test_env.context.clone(),
            waiter: mock_waiter,
        };
        let test_buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<CommandStatus>::new_test(
            Some(Format::JsonPretty),
            &test_buffers,
        );
        let res = tool.main(writer).await;
        let (stdout, stderr) = test_buffers.into_strings();
        assert!(res.is_err(), "expected error {stdout} {stderr}");
        if let Err(WaitError::WaitFailed(err_msg)) = res {
            assert!(
                err_msg.contains("fastboot timeout"),
                "expected 'fastboot timeout' in error message: {err_msg}"
            );
            assert!(
                !err_msg.contains("Diagnostics:"),
                "did not expect 'Diagnostics:' in error message: {err_msg}"
            );
        } else {
            panic!("expected WaitFailed, got: {:?}", res);
        }
    }

    #[fuchsia::test]
    async fn test_product_error_includes_diagnostics() {
        let test_env = ffx_config::test_init().expect("test env");
        let mut mock_waiter = MockDeviceWaiter::new();
        mock_waiter
            .expect_wait()
            .times(1)
            .returning(|_, _, _, _| Box::pin(async { Err(fho::bug!("product timeout")) }));
        let tool = WaitOperation {
            cmd: WaitOptions {
                timeout: 1000,
                down: false,
                state: Some(TargetStateOption::Product),
            },
            env: test_env.context.clone(),
            waiter: mock_waiter,
        };
        let test_buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<CommandStatus>::new_test(
            Some(Format::JsonPretty),
            &test_buffers,
        );
        let res = tool.main(writer).await;
        let (stdout, stderr) = test_buffers.into_strings();
        assert!(res.is_err(), "expected error {stdout} {stderr}");
        if let Err(WaitError::WaitFailed(err_msg)) = res {
            assert!(
                err_msg.contains("product timeout"),
                "expected 'product timeout' in error message: {err_msg}"
            );
            assert!(
                err_msg.contains("Diagnostics:"),
                "expected 'Diagnostics:' in error message: {err_msg}"
            );
        } else {
            panic!("expected WaitFailed, got: {:?}", res);
        }
    }

    #[fuchsia::test]
    async fn test_default_wait_options() {
        let options = WaitOptions::default();
        assert_eq!(options.timeout, 120);
        assert!(!options.down);
        assert_eq!(options.state, None);
        assert_eq!(options.get_target_state(), Ok(TargetStateOption::Up));
    }

    #[fuchsia::test]
    async fn test_state_option_display() {
        assert_eq!(TargetStateOption::Down.to_string(), "down");
        assert_eq!(TargetStateOption::Up.to_string(), "up");
        assert_eq!(TargetStateOption::Fastboot.to_string(), "fastboot");
        assert_eq!(TargetStateOption::Product.to_string(), "product");
    }

    #[fuchsia::test]
    async fn test_cli_args_mutual_exclusivity_from_args() {
        for state_str in ["down", "up", "fastboot", "product"] {
            let options = WaitOptions::from_args(&["wait"], &["--down", "--state", state_str])
                .expect("parse options");
            assert!(options.get_target_state().is_err());

            let options_short = WaitOptions::from_args(&["wait"], &["-d", "--state", state_str])
                .expect("parse options with -d");
            assert!(options_short.get_target_state().is_err());
        }
    }

    #[fuchsia::test]
    async fn test_get_diagnostics_string_escapes_control_characters() {
        let env = ffx_config::test_env()
            .runtime_config(ffx_config::keys::TARGET_DEFAULT_KEY, "target\x1b[31m_malicious\r\0")
            .build()
            .unwrap();
        let diag_str = get_diagnostics_string(&env.context, 1, fho::bug!("wait failed")).await;
        assert!(diag_str.contains("wait failed"));
        assert!(diag_str.contains("Diagnostics:"));
        assert!(!diag_str.contains('\x1b'));
        assert!(!diag_str.contains('\0'));
        assert!(!diag_str.contains('\r'));
        assert!(diag_str.contains("target\\u{1b}[31m_malicious\\r\\u{0}"));
    }
}
