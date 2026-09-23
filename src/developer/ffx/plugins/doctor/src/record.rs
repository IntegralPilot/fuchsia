// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::environment::get_user_config;
use crate::types::{DoctorRecorderParameters, DoctorStepHandler, StepResult, StepType};
use anyhow::{Context, Result, anyhow};
use ffx_config::EnvironmentContext;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime};

const PLATFORM_INFO_FILENAME: &str = "platform.json";
const USER_CONFIG_FILENAME: &str = "user_config.txt";

pub fn get_kernel_name() -> Result<String> {
    Ok(String::from_utf8(Command::new("uname").output()?.stdout)?)
}

pub fn get_platform_info() -> Result<String> {
    let kernel_name = match get_kernel_name() {
        Ok(s) => s,
        Err(e) => format!("Could not get kernel name: {}", e),
    };

    let platform_info = json!({
        "kernel_name": kernel_name.trim(),
    });

    Ok(serde_json::to_string_pretty(&platform_info)?)
}

pub async fn doctor_record(
    ctx: &EnvironmentContext,
    step_handler: &mut impl DoctorStepHandler,
    record_params: DoctorRecorderParameters,
) -> Result<()> {
    let log_root = record_params
        .log_root
        .clone()
        .context("log_root not present despite record set to true")?;
    let output_dir = record_params
        .output_dir
        .clone()
        .context("output_dir not present despite record set to true")?;

    let log_files: Vec<PathBuf> = collect_log_files(log_root.clone())?;

    step_handler.step(StepType::GeneratingRecord).await?;

    let platform_info = match get_platform_info() {
        Ok(s) => s,
        Err(e) => format!("Could not serialize platform info: {}", e),
    };

    let final_path = {
        let mut r = record_params.recorder.lock().await;
        r.add_sources(log_files);
        r.add_content(PLATFORM_INFO_FILENAME, platform_info);

        if record_params.user_config_enabled {
            let config_str = match get_user_config(ctx) {
                Ok(s) => s,
                Err(e) => format!("Could not get config data output: {}", e),
            };
            r.add_content(USER_CONFIG_FILENAME, config_str);
        }

        match r.generate(output_dir.clone()) {
            Ok(p) => p,
            Err(e) => {
                let path = &output_dir.to_str().unwrap_or("path undefined");
                let advice = "You can change the output directory for the generated zip file \
                                  using `--output-dir`.";
                let default_err_msg =
                    Err(anyhow!("{}\nCould not write to: {}\n{}", e, path, advice));

                match &e {
                    doctor_utils::DoctorUtilsError::Zip(zip::result::ZipError::Io(io_error)) => {
                        match io_error.raw_os_error() {
                            Some(27) => Err(anyhow!(
                                "{}\nMake sure you can write files larger than 1MB to: {}\n{}",
                                e,
                                path,
                                advice
                            ))?,
                            _ => default_err_msg?,
                        }
                    }
                    _ => default_err_msg?,
                }
            }
        }
    };

    step_handler.result(StepResult::Success).await?;
    step_handler.output_step(StepType::RecordGenerated(final_path.canonicalize()?)).await?;
    Ok(())
}

pub fn collect_log_files(root_dir: PathBuf) -> Result<Vec<PathBuf>> {
    let now = SystemTime::now();
    // Get all log files that have been modified recently.
    const NINETY_DAYS_SECS: u64 = 60 * 60 * 24 * 90;
    const MAX_AGE: Duration = Duration::from_secs(NINETY_DAYS_SECS);

    let list = root_dir
        .read_dir()?
        .filter_map(|entry| {
            if let Ok(d) = entry {
                Some(d.path())
            } else {
                log::debug!("Skipping read dir was an error: {entry:?}");
                None
            }
        })
        .filter_map(|p| {
            if p.is_dir() {
                log::debug!("Skipping dir {:?}", p);
                None
            } else {
                Some(p)
            }
        })
        .filter(|p| {
            if p.extension().unwrap_or_default() == "log" {
                true
            } else {
                log::debug!("Skipping non .log extension {:?}", p);
                false
            }
        })
        .filter_map(|p| match fs::metadata(p.clone()) {
            Ok(mdata) => Some((p, mdata)),
            Err(e) => {
                log::error!("could not read metadata for {:?}: {e}", p);
                None
            }
        })
        .filter_map(|(p, mdata)| match mdata.modified() {
            Ok(mdate) => Some((p, mdate)),
            Err(e) => {
                log::error!("could not read modified time for {:?}: {e}", p);
                None
            }
        })
        .filter_map(|(p, mdate)| match now.duration_since(mdate) {
            Ok(age) => {
                if age < MAX_AGE {
                    Some(p)
                } else {
                    log::debug!("Skipping {p:?} too  old {}", age.as_secs());
                    None
                }
            }
            Err(e) => {
                log::error!("could not determine duration {p:?}: {e}");
                None
            }
        })
        .collect();
    Ok(list)
}
