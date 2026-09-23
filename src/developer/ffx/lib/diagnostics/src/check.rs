// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::future::Future;
use std::pin::Pin;

pub type CheckResult<T> = Result<T, anyhow::Error>;

pub type CheckFut<'a, T> = Pin<Box<dyn Future<Output = CheckResult<T>> + 'a>>;

pub trait Check {
    /// The input for the next check in the check stream.
    type Input;

    /// The output for the next check to take as input.
    type Output;

    /// The type of the writer used in each check. Defaults to a vector of bytes for testing, but
    /// this should typically be used with subtools, so one would pass the tool's `::Notifier` to
    /// this.
    type Notifier: Notifier;

    /// Optional write before the check is run.
    fn write_preamble(
        &self,
        _input: &Self::Input,
        _notifier: &mut Self::Notifier,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Optional write on success of check.
    fn on_success(
        &self,
        _output: &Self::Output,
        _notifier: &mut Self::Notifier,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Required: the actual check being implemented.
    fn check<'a>(
        &'a mut self,
        input: Self::Input,
        notifier: &'a mut Self::Notifier,
    ) -> CheckFut<'a, Self::Output>;

    /// Auto-implemented. Runs a check while writing a preamble. Returns a tuple of the main output
    /// and the reference to the writer being used.
    ///
    /// Allows for chaining checks together.
    fn check_with_notifier<'a>(
        mut self,
        input: Self::Input,
        notifier: &'a mut Self::Notifier,
    ) -> CheckFut<'a, (Self::Output, &'a mut Self::Notifier)>
    where
        Self::Notifier: Sized,
        Self: Sized + 'a,
    {
        Box::pin(async move {
            self.write_preamble(&input, notifier)?;
            let result = (self.check(input, notifier).await?, notifier);
            self.on_success(&result.0, result.1)?;
            Ok(result)
        })
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub enum NotificationType {
    Info,
    Success,
    Warning,
    Error,
}

impl fmt::Display for NotificationType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            NotificationType::Info => write!(f, "INFO")?,
            NotificationType::Success => write!(f, "SUCCESS")?,
            NotificationType::Warning => write!(f, "WARNING")?,
            NotificationType::Error => write!(f, "ERROR")?,
        }
        Ok(())
    }
}

/// A trait for updating the progress of a check as it continues.
pub trait Notifier {
    fn update_status(
        &mut self,
        ty: NotificationType,
        status: impl Into<String>,
    ) -> anyhow::Result<()>;

    fn warn(&mut self, status: impl Into<String>) -> anyhow::Result<()> {
        self.update_status(NotificationType::Warning, status)
    }

    fn info(&mut self, status: impl Into<String>) -> anyhow::Result<()> {
        self.update_status(NotificationType::Info, status)
    }

    fn on_error(&mut self, status: impl Into<String>) -> anyhow::Result<()> {
        self.update_status(NotificationType::Error, status)
    }

    fn on_success(&mut self, status: impl Into<String>) -> anyhow::Result<()> {
        self.update_status(NotificationType::Success, status)
    }
}

/// Simple Notifier when the goal is to just collect the messages into a String
#[derive(Debug)]
pub struct StringNotifier(String);

impl StringNotifier {
    pub fn new() -> Self {
        Self(String::new())
    }
}

impl From<StringNotifier> for String {
    fn from(value: StringNotifier) -> Self {
        value.0
    }
}

impl Notifier for StringNotifier {
    fn update_status(
        &mut self,
        ty: NotificationType,
        status: impl Into<String>,
    ) -> anyhow::Result<()> {
        let msg: String = status.into();
        let line = format!("\n{ty:>7}: {msg}");
        self.0.push_str(&line);
        Ok(())
    }
}
