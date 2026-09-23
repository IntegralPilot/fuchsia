// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::doctor_ledger::LedgerNode;
use anyhow::Result;
use async_lock::Mutex;
use async_trait::async_trait;
use doctor_utils::Recorder;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use termio::Colors;

pub const DOCTOR_OUTPUT_FILENAME: &str = "doctor_output.txt";

#[derive(serde::Serialize)]
pub struct DoctorResult {
    pub steps: LedgerNode,
}

#[derive(Debug, PartialEq, Clone)]
pub enum StepType {
    DoctorSummaryInitNormal,
    DoctorSummaryInitVerbose,
    GeneratingRecord,
    Output(String),
    RecordGenerated(PathBuf),
    DoctorNoticeWarning,
    DoctorNoticeFailure,
}

#[derive(Debug, Clone)]
pub enum StepResult {
    Success,
}

impl std::fmt::Display for StepResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StepResult::Success => write!(f, "success"),
        }
    }
}

impl std::fmt::Display for StepType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StepType::DoctorSummaryInitNormal => {
                write!(f, "\nDoctor summary (to see all details, run ffx doctor -v):\n")
            }
            StepType::DoctorSummaryInitVerbose => {
                write!(f, "\nDoctor summary:\n")
            }
            StepType::GeneratingRecord => write!(f, "Generating record..."),
            StepType::Output(data_str) => write!(f, "{}", data_str),
            StepType::RecordGenerated(path) => {
                write!(f, "Record generated at: {}\n", path.to_string_lossy())
            }
            StepType::DoctorNoticeWarning => write!(
                f,
                "Warning: ffx doctor detected potential anomalies. Review the items marked with [!] above."
            ),
            StepType::DoctorNoticeFailure => write!(
                f,
                "Error: ffx doctor detected operational failures. Resolve the items marked with [✗] above."
            ),
        }
    }
}

impl StepType {
    pub fn format(&self, colors: &Colors) -> String {
        match self {
            StepType::DoctorSummaryInitNormal => {
                format!(
                    "\n{}Doctor summary (to see all details, run ffx doctor -v):{}\n",
                    colors.bold, colors.reset
                )
            }
            StepType::DoctorSummaryInitVerbose => {
                format!("\n{}Doctor summary:{}\n", colors.bold, colors.reset)
            }
            _ => format!("{}", self),
        }
    }
}

#[async_trait]
pub trait DoctorStepHandler {
    async fn step(&mut self, step: StepType) -> Result<()>;
    async fn output_step(&mut self, step: StepType) -> Result<()>;
    async fn record(&mut self, step: StepType) -> Result<()>;
    async fn result(&mut self, result: StepResult) -> Result<()>;
}

pub struct DefaultDoctorStepHandler {
    pub recorder: Arc<Mutex<dyn Recorder + Send>>,
    pub writer: Box<dyn Write + Send + Sync>,
    pub colors: Colors,
}

#[async_trait]
impl DoctorStepHandler for DefaultDoctorStepHandler {
    async fn step(&mut self, step: StepType) -> Result<()> {
        write!(&mut self.writer, "{}", step.format(&self.colors))?;
        self.writer.flush()?;
        let mut r = self.recorder.lock().await;
        r.add_content(DOCTOR_OUTPUT_FILENAME, step.format(&Colors::disabled()));
        Ok(())
    }

    async fn output_step(&mut self, step: StepType) -> Result<()> {
        writeln!(&mut self.writer, "{}", step.format(&self.colors))?;
        let mut r = self.recorder.lock().await;
        r.add_content(DOCTOR_OUTPUT_FILENAME, format!("{}\n", step.format(&Colors::disabled())));
        Ok(())
    }

    async fn record(&mut self, step: StepType) -> Result<()> {
        let mut r = self.recorder.lock().await;
        r.add_content(DOCTOR_OUTPUT_FILENAME, step.format(&Colors::disabled()));
        Ok(())
    }

    async fn result(&mut self, result: StepResult) -> Result<()> {
        writeln!(&mut self.writer, "{}", result)?;
        let mut r = self.recorder.lock().await;
        r.add_content(DOCTOR_OUTPUT_FILENAME, format!("{}\n", result));
        Ok(())
    }
}

impl DefaultDoctorStepHandler {
    pub fn new(
        recorder: Arc<Mutex<dyn Recorder + Send>>,
        writer: Box<dyn Write + Send + Sync>,
        colors: Colors,
    ) -> Self {
        Self { recorder, writer, colors }
    }
}

pub struct DoctorRecorderParameters {
    pub record: bool,
    pub user_config_enabled: bool,
    pub log_root: Option<PathBuf>,
    pub output_dir: Option<PathBuf>,
    pub recorder: Arc<Mutex<dyn Recorder>>,
}

pub fn get_api_level(api_level: Option<u64>) -> String {
    match api_level {
        Some(api) => format!("{}", api),
        None => "UNKNOWN".to_string(),
    }
}

pub fn get_abi_revision(revision: Option<u64>) -> String {
    match revision {
        Some(abi) => format!("{:#X}", abi),
        None => "UNKNOWN".to_string(),
    }
}
