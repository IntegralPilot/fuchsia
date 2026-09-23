// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::Error;
use analytics::{get_notice, opt_out_for_this_invocation};
use ffx_config::EnvironmentContext;
use ffx_metrics::{add_ffx_launch_event, enhanced_analytics, init_metrics_svc};
use fuchsia_async::TimeoutExt;
use itertools::Itertools;
use regex::Regex;
use std::io::Write;
use std::process::ExitStatus;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use crate::{FfxContext, Result};

const UNKNOWN_SDK: &str = "Unknown SDK";
pub struct MetricsSession {
    enabled: bool,
    upload_timeout: Duration,
    session_start: Instant,
    connection_mode: &'static str,
}

pub struct CommandStats {
    pub success: bool,
    pub command_duration: Duration,
    pub analytics_duration: Option<Duration>,
}

impl std::fmt::Display for CommandStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self { success, command_duration, analytics_duration } = self;
        write!(f, "Success: {success}, exec time: {:.4}, ", command_duration.as_secs_f32())?;
        match analytics_duration {
            Some(analytics_duration) => {
                write!(f, "analytics time: {:.4}", analytics_duration.as_secs_f32())
            }
            None => write!(f, "analytics disabled"),
        }
    }
}

impl MetricsSession {
    pub async fn start(context: &EnvironmentContext) -> Result<Self> {
        let upload_timeout = crate::get_upload_timeout(Some(context));
        let invoker = context.get("fuchsia.analytics.ffx_invoker").unwrap_or(None);
        let build_info = context.build_info();
        let enabled = context.analytics_enabled();
        let analytics_path = context.get_analytics_path();
        let sdk_version = if enabled {
            get_sdk_version(&context).unwrap_or_else(|| UNKNOWN_SDK.to_string())
        } else {
            UNKNOWN_SDK.to_string()
        };
        init_metrics_svc(analytics_path, build_info, invoker.clone(), sdk_version).await;
        let call_stack = std::env::var("FUCHSIA_METRICS_CALL_STACK").unwrap_or_default();
        let new_call_stack =
            if call_stack.is_empty() { "ffx".to_string() } else { format!("{call_stack} ffx") };
        // SAFETY: This is called during single-threaded initialization before any other threads are spawned.
        unsafe {
            std::env::set_var("FUCHSIA_METRICS_CALL_STACK", new_call_stack);
        }
        if !enabled {
            opt_out_for_this_invocation().await.map_err(anyhow::Error::from)?
        }
        let connection_mode = if context.is_strict() {
            "strict"
        } else if context.get_direct_connection_mode() {
            "direct"
        } else {
            "daemon"
        };
        let session_start = Instant::now();
        Ok(Self { enabled, session_start, upload_timeout, connection_mode })
    }

    pub async fn print_notice(&self, out: &mut impl Write) -> Result<()> {
        if let Some(note) = get_notice().await {
            writeln!(out, "{}", note).bug()?;
        }
        Ok(())
    }

    pub async fn command_finished(
        self,
        res: &Result<ExitStatus, Error>,
        redacted_args: &[impl AsRef<str>],
        enhanced_args: Option<&[impl AsRef<str>]>,
    ) -> Result<CommandStats> {
        let exit_code = match res {
            Ok(c) => c.code().unwrap_or(1),
            Err(e) => e.exit_code(),
        };
        let error_message = match res {
            Ok(_) => None,
            Err(e) => Some(e.to_string()),
        };

        let command_done = Instant::now();
        let command_duration = command_done - self.session_start;
        let analytics_duration = if self.enabled {
            let timing_in_millis = command_duration.as_millis();
            let redacted_args = redacted_args.iter().map(AsRef::as_ref).join(" ");
            let enhanced_args = enhanced_args.map(|val| val.iter().map(AsRef::as_ref).join(" "));
            let connection_mode = self.connection_mode;
            let analytics_done = async move {
                if let Err(e) = add_ffx_launch_event(
                    Some(connection_mode),
                    redacted_args,
                    enhanced_args,
                    timing_in_millis,
                    exit_code,
                    error_message,
                )
                .await
                {
                    log::error!("metrics submission failed: {}", e);
                }
                Instant::now()
            }
            .on_timeout(self.upload_timeout, || {
                log::error!("metrics submission timed out");
                // Metrics timeouts should not impact user flows.
                Instant::now()
            })
            .await;
            let analytics_duration = analytics_done - command_done;

            Some(analytics_duration)
        } else {
            None
        };
        let success = exit_code == 0;
        let stats = CommandStats { success, command_duration, analytics_duration };
        match success {
            true => log::info!("{stats}",),
            false => log::warn!("{stats}",),
        }
        Ok(stats)
    }
}

pub async fn send_enhanced_analytics() -> bool {
    enhanced_analytics().await
}

fn get_sdk_version(context: &EnvironmentContext) -> Option<String> {
    match context.get_sdk() {
        Ok(sdk) => sdk.get_version_string(),
        Err(_) => None,
    }
}

pub fn analytics_command(command_str: &str) -> bool {
    static ANALYTICS_COMMAND_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(" config analytics ").unwrap());
    ANALYTICS_COMMAND_RE.is_match(command_str)
}
