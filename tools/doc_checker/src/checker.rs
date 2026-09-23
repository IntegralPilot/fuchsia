// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! checker are the traits and structs used to perform checks on markdown documentation
//! in the Fuchsia project.

use crate::md_element::Element;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_yaml::Value;
use std::collections::{HashMap, HashSet};
use std::fmt::{self, Debug, Display};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub type ReachabilityGraph = Arc<Mutex<HashMap<PathBuf, HashSet<PathBuf>>>>;
pub type ExemptionSet = Arc<Mutex<HashSet<PathBuf>>>;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Ord, PartialOrd, Serialize)]
pub enum ErrorLevel {
    Info,
    Warning,
    Error,
}
impl fmt::Display for ErrorLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            ErrorLevel::Info => write!(f, "Info"),
            ErrorLevel::Warning => write!(f, "Warning"),
            ErrorLevel::Error => write!(f, "Error"),
        }
    }
}

/// An error reported by a [`DocCheck`].
#[derive(Clone, Deserialize, Debug, Eq, Ord, PartialOrd, PartialEq, Serialize)]
pub struct DocCheckError {
    pub level: ErrorLevel,
    pub doc_line: DocLine,
    pub message: String,
    pub help_suggestion: Option<String>,
}

impl DocCheckError {
    pub fn new_error(line_num: usize, file_name: PathBuf, message: &str) -> Self {
        DocCheckError {
            doc_line: DocLine { line_num, file_name },
            message: message.to_string(),
            help_suggestion: None,
            level: ErrorLevel::Error,
        }
    }
    pub fn new_error_helpful(
        line_num: usize,
        file_name: PathBuf,
        message: &str,
        help: &str,
    ) -> Self {
        DocCheckError {
            doc_line: DocLine { line_num, file_name },
            message: message.to_string(),
            help_suggestion: Some(help.to_string()),
            level: ErrorLevel::Error,
        }
    }
    pub fn new_warning(line_num: usize, file_name: PathBuf, message: &str) -> Self {
        DocCheckError {
            doc_line: DocLine { line_num, file_name },
            message: message.to_string(),
            help_suggestion: None,
            level: ErrorLevel::Warning,
        }
    }
    pub fn new_warning_helpful(
        line_num: usize,
        file_name: PathBuf,
        message: &str,
        help: &str,
    ) -> Self {
        DocCheckError {
            doc_line: DocLine { line_num, file_name },
            message: message.to_string(),
            help_suggestion: Some(help.to_string()),
            level: ErrorLevel::Warning,
        }
    }
    pub fn new_info(line_num: usize, file_name: PathBuf, message: &str) -> Self {
        DocCheckError {
            doc_line: DocLine { line_num, file_name },
            message: message.to_string(),
            help_suggestion: None,
            level: ErrorLevel::Info,
        }
    }
    pub fn new_info_helpful(
        line_num: usize,
        file_name: PathBuf,
        message: &str,
        help: &str,
    ) -> Self {
        DocCheckError {
            doc_line: DocLine { line_num, file_name },
            message: message.to_string(),
            help_suggestion: Some(help.to_string()),
            level: ErrorLevel::Info,
        }
    }
}

impl fmt::Display for DocCheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let suggestion = self
            .help_suggestion
            .as_ref()
            .map(|suggestion| format!("\nConsider {}", suggestion))
            .unwrap_or(String::new());

        f.write_fmt(format_args!(
            "{level}\n{doc_line}\n{message}{suggestion}",
            level = self.level,
            doc_line = self.doc_line,
            message = self.message,
            suggestion = suggestion
        ))
    }
}

/// A line within a file.
#[derive(Debug, Deserialize, Clone, Eq, Hash, Ord, PartialOrd, PartialEq, Serialize)]
pub struct DocLine {
    pub file_name: PathBuf,
    pub line_num: usize,
}

impl Display for DocLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("{}:{}", self.file_name.to_string_lossy(), self.line_num))
    }
}

/// Trait for DocCheck. Implementations of this trait are collected into a list
/// and check() is called for each event during parsing the markdown.
#[async_trait]
pub trait DocCheck {
    /// Name for this check. This name is used in error messages to identify the source
    /// of the problem.
    fn name(&self) -> &str;

    /// Given the event, determine if this check applies to the event and return a list
    /// of errors if any are detected.
    /// None should be returned if the check does not apply to this event,
    /// or if no errors are found.
    /// The reference to self is mut, which enables checks to collection information as
    /// each file is checked and then consulted in the post_check.
    fn check(&mut self, element: &Element<'_>) -> Result<Option<Vec<DocCheckError>>>;

    /// Some checks require visiting all pages, the post_check method is called after all
    /// markdown has been parsed.
    async fn post_check(&self) -> Result<Option<Vec<DocCheckError>>>;
}

/// Trait for DocCheck to use with yaml files. Implementations of this trait are collected
/// into a list and check() is called for each event during parsing the yaml.
#[async_trait]
pub trait DocYamlCheck {
    /// Name for this check. This name is used in error messages to identify the source
    /// of the problem.
    fn name(&self) -> &str;

    /// Given the yaml, determine if this check applies to the event and return a list
    /// of errors if any are detected.
    /// An empty list of errors should be returned if the check does not apply to this event,
    /// or if no errors are found.
    fn check(&mut self, filename: &Path, yaml_value: &Value) -> Result<Option<Vec<DocCheckError>>>;

    /// Some checks require visiting all pages, the post_check method is called after all
    /// markdown has been parsed.
    async fn post_check(
        &self,
        markdown_files: &[PathBuf],
        yaml_files: &[PathBuf],
    ) -> Result<Option<Vec<DocCheckError>>>;
}
