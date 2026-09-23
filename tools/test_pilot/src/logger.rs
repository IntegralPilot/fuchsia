// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::name::Name;
use serde_json::Value;
use std::path::PathBuf;

// TODO(https://fxbug.dev/395959242) Use log crate for StderrLogger or to replace Logger entirely.

/// Logger trait for logging builder operation.
pub trait Logger {
    fn strict(&mut self);

    fn start_command_line(&mut self);

    fn start_include(&mut self, path: &PathBuf);

    fn add_include(&mut self, path: &PathBuf);

    fn include_already_added(&mut self, path: &PathBuf);

    fn add_require(&mut self, parameter_name: &Name);

    fn require_already_added(&mut self, parameter_name: &Name);

    fn add_prohibit(&mut self, parameter_name: &Name);

    fn prohibit_already_added(&mut self, parameter_name: &Name);

    fn schema_option(&mut self, path: &str);

    fn debug_option(&mut self);

    fn add_parameter_non_strict(&mut self, name: &Name, value: &Value);

    fn add_parameter_strict(&mut self, name: &Name, value: &Value);

    fn overridden_add_parameter_strict_ignored(&mut self, name: &Name, value: &Value);

    fn add_to_array(&mut self, name: &Name, value: &Value);

    fn from_env(&mut self, var_name_value: &Value, value: &Value);

    fn try_from_env(&mut self, var_name_value: &Value, value: &Value);

    fn try_from_env_undefined(&mut self, name: &Name, var_name_value: &Value);
}

#[allow(dead_code)] // TODO(https://fxbug.dev/421409143)
/// Logger that does nothing.
pub struct NullLogger;

impl Logger for NullLogger {
    fn strict(&mut self) {}

    fn start_command_line(&mut self) {}

    fn start_include(&mut self, _path: &PathBuf) {}

    fn add_include(&mut self, _path: &PathBuf) {}

    fn add_require(&mut self, _parameter_name: &Name) {}

    fn require_already_added(&mut self, _parameter_name: &Name) {}

    fn add_prohibit(&mut self, _parameter_name: &Name) {}

    fn prohibit_already_added(&mut self, _parameter_name: &Name) {}

    fn include_already_added(&mut self, _path: &PathBuf) {}

    fn schema_option(&mut self, _path: &str) {}

    fn debug_option(&mut self) {}

    fn add_parameter_non_strict(&mut self, _name: &Name, _value: &Value) {}

    fn add_parameter_strict(&mut self, _name: &Name, _value: &Value) {}

    fn overridden_add_parameter_strict_ignored(&mut self, _name: &Name, _value: &Value) {}

    fn add_to_array(&mut self, _name: &Name, _value: &Value) {}

    fn from_env(&mut self, _var_name_value: &Value, _value: &Value) {}

    fn try_from_env(&mut self, _var_name_value: &Value, _value: &Value) {}

    fn try_from_env_undefined(&mut self, _name: &Name, _var_name_value: &Value) {}
}

/// Logger that prints messages to stderr.
pub struct StderrLogger;

impl Logger for StderrLogger {
    fn strict(&mut self) {
        eprintln!("strict");
    }

    fn start_command_line(&mut self) {
        eprintln!("from command line:");
    }

    fn start_include(&mut self, path: &PathBuf) {
        eprintln!("from {}", path.to_str().unwrap());
    }

    fn add_include(&mut self, path: &PathBuf) {
        eprintln!("    include {}", path.to_str().unwrap());
    }

    fn include_already_added(&mut self, path: &PathBuf) {
        eprintln!("    already included {}", path.to_str().unwrap());
    }

    fn add_require(&mut self, parameter_name: &Name) {
        eprintln!("    require {}", parameter_name);
    }

    fn require_already_added(&mut self, parameter_name: &Name) {
        eprintln!("    already required {}", parameter_name);
    }

    fn add_prohibit(&mut self, parameter_name: &Name) {
        eprintln!("    prohibit {}", parameter_name);
    }

    fn prohibit_already_added(&mut self, parameter_name: &Name) {
        eprintln!("    already prohibited {}", parameter_name);
    }

    fn schema_option(&mut self, path: &str) {
        eprintln!("    schema {}", path);
    }

    fn debug_option(&mut self) {}

    fn add_parameter_non_strict(&mut self, name: &Name, value: &Value) {
        eprintln!("    set {} = {:?}", name, value);
    }

    fn add_parameter_strict(&mut self, name: &Name, value: &Value) {
        eprintln!("    strict set {} = {:?}", name, value);
    }

    fn overridden_add_parameter_strict_ignored(&mut self, name: &Name, value: &Value) {
        eprintln!("    ignoring overridden strict set {} = {:?}", name, value);
    }

    fn add_to_array(&mut self, name: &Name, value: &Value) {
        eprintln!("    add to {} {:?}", name, value.as_array().expect("value is array"));
    }

    fn from_env(&mut self, var_name_value: &Value, value: &Value) {
        eprintln!(
            "     from env var {} got value {:?}",
            var_name_value.as_str().expect("var_name_value is string"),
            value
        );
    }

    fn try_from_env(&mut self, var_name_value: &Value, value: &Value) {
        eprintln!(
            "     try from env var {} got value {:?}",
            var_name_value.as_str().expect("var_name_value is string"),
            value
        );
    }

    fn try_from_env_undefined(&mut self, name: &Name, var_name_value: &Value) {
        eprintln!(
            "     try from env var {} undefined, skipping assignment to {}",
            var_name_value.as_str().expect("var_name_value is string"),
            name
        );
    }
}

pub struct RetainingLogger {
    messages: Vec<String>,
}

impl RetainingLogger {
    pub fn new() -> Self {
        Self { messages: vec![] }
    }

    pub fn eprintln(&self) {
        for message in &self.messages {
            eprintln!("{0}", message);
        }
    }
}

impl Logger for RetainingLogger {
    fn strict(&mut self) {
        self.messages.push(format!("strict"));
    }

    fn start_command_line(&mut self) {
        self.messages.push(format!("from command line:"));
    }

    fn start_include(&mut self, path: &PathBuf) {
        self.messages.push(format!("from {}", path.to_str().unwrap()));
    }

    fn add_include(&mut self, path: &PathBuf) {
        self.messages.push(format!("    include {}", path.to_str().unwrap()));
    }

    fn include_already_added(&mut self, path: &PathBuf) {
        self.messages.push(format!("    already included {}", path.to_str().unwrap()));
    }

    fn add_require(&mut self, parameter_name: &Name) {
        self.messages.push(format!("    require {}", parameter_name));
    }

    fn require_already_added(&mut self, parameter_name: &Name) {
        self.messages.push(format!("    already required {}", parameter_name));
    }

    fn add_prohibit(&mut self, parameter_name: &Name) {
        self.messages.push(format!("    prohibit {}", parameter_name));
    }

    fn prohibit_already_added(&mut self, parameter_name: &Name) {
        self.messages.push(format!("    already prohibited {}", parameter_name));
    }

    fn schema_option(&mut self, path: &str) {
        self.messages.push(format!("    schema {}", path));
    }

    fn debug_option(&mut self) {}

    fn add_parameter_non_strict(&mut self, name: &Name, value: &Value) {
        self.messages.push(format!("    set {} = {:?}", name, value));
    }

    fn add_parameter_strict(&mut self, name: &Name, value: &Value) {
        self.messages.push(format!("    strict set {} = {:?}", name, value));
    }

    fn overridden_add_parameter_strict_ignored(&mut self, name: &Name, value: &Value) {
        self.messages.push(format!("    ignoring overridden strict set {} = {:?}", name, value));
    }

    fn add_to_array(&mut self, name: &Name, value: &Value) {
        self.messages.push(format!(
            "    add to {} {:?}",
            name,
            value.as_array().expect("value is array")
        ));
    }

    fn from_env(&mut self, var_name_value: &Value, value: &Value) {
        self.messages.push(format!(
            "    from env var {} got value {:?}",
            var_name_value.as_str().expect("var_name_value is string"),
            value
        ));
    }

    fn try_from_env(&mut self, var_name_value: &Value, value: &Value) {
        self.messages.push(format!(
            "    try from env var {} got value {:?}",
            var_name_value.as_str().expect("var_name_value is string"),
            value
        ));
    }

    fn try_from_env_undefined(&mut self, name: &Name, var_name_value: &Value) {
        self.messages.push(format!(
            "    try from env var {} undefined, skipping assignment to {}",
            var_name_value.as_str().expect("var_name_value is string"),
            name
        ));
    }
}
