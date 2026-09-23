// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A fake for ToolProvider that can be used in tests.

use crate::tool::{Tool, ToolCommand, ToolCommandLog, ToolProvider};
use anyhow::Result;
use assembly_util::write_json_file;
use serde_json::json;
use std::path::{Path, PathBuf};
use utf8_path::PathToStringExt;

/// A provider for tools that no-op, but log their execution so it can be asserted in a test.
#[derive(Clone)]
pub struct FakeToolProvider {
    /// A log of all the tools that were run.
    log: ToolCommandLog,
    /// Function called for each tool invocation.
    side_effect: fn(&str, &[String]) -> (),
}

/// A default side-effect when running a fake blobfs tool.
/// This side-effect will write the blobs.json file.
pub fn blobfs_side_effect(_name: &str, args: &[String]) {
    assert_eq!(args[0], "--json-output");
    write_json_file(
        Path::new(&args[1]),
        &json!([{
            "merkle": "b62ee413090825c2ae70fe143b34cbd851f055932cfd5e7ca4ef0efbb802da2a",
            "size": 73i32
        }]),
    )
    .unwrap();
}

impl FakeToolProvider {
    /// Creates a tool provider that notifies a function for each invocation.
    pub fn new_with_side_effect(side_effect: fn(&str, &[String]) -> ()) -> FakeToolProvider {
        FakeToolProvider { log: ToolCommandLog::default(), side_effect }
    }

    /// Get a tool by its name.
    fn get_tool_with_name(&self, name: String) -> Result<Box<dyn Tool>> {
        Ok(Box::new(FakeTool { name, log: self.log().clone(), side_effect: self.side_effect }))
    }
}

impl Default for FakeToolProvider {
    fn default() -> FakeToolProvider {
        fn noop(_: &str, _: &[String]) -> () {}
        FakeToolProvider::new_with_side_effect(noop)
    }
}

impl ToolProvider for FakeToolProvider {
    fn get_tool(&self, name: &str) -> Result<Box<dyn Tool>> {
        self.get_tool_with_name(format!("./host_x64/{}", name))
    }

    fn get_tool_with_path(&self, path: PathBuf) -> Result<Box<dyn Tool>> {
        self.get_tool_with_name(path.path_to_string()?)
    }

    fn log(&self) -> &ToolCommandLog {
        &self.log
    }

    fn get_tool_path(&self, name: &str) -> Result<PathBuf> {
        Ok(PathBuf::from(format!("./host_x64/{}", name)))
    }
}

/// A fake tool that does not execute a command, but only writes it to the |log|.
#[derive(Clone)]
struct FakeTool {
    /// The name of the tool.
    name: String,
    /// The log to write the execution details into upon a `run()`.
    log: ToolCommandLog,
    /// Function called for each tool invocation.
    side_effect: fn(&str, &[String]) -> (),
}

impl Tool for FakeTool {
    fn run(&self, args: &[String]) -> Result<()> {
        (self.side_effect)(&self.name, args);
        self.log.add(ToolCommand::new(self.name.clone(), args.into()));
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::FakeToolProvider;
    use crate::tool::{ToolCommandLog, ToolProvider};
    use serde_json::json;

    #[test]
    fn test_fake_tool_provider() {
        let expected_log: ToolCommandLog = serde_json::from_value(json!({
            "commands": [
                {
                    "tool": "./host_x64/my_tool",
                    "args": [
                        "foo",
                        "bar",
                    ]
                },
                {
                    "tool": "./host_x64/my_other_tool",
                    "args": [
                        "cat"
                    ]
                },
                {
                    "tool": "my_tool_with_path",
                    "args": [
                        "cat"
                    ]
                },
            ]
        }))
        .unwrap();

        let tools = FakeToolProvider::default();

        let tool = tools.get_tool("my_tool").unwrap();
        assert!(tool.run(&["foo".into(), "bar".into()]).is_ok());
        drop(tool);

        let tool = tools.get_tool("my_other_tool").unwrap();
        assert!(tool.run(&["cat".into()]).is_ok());
        drop(tool);

        let tool = tools.get_tool_with_path("my_tool_with_path".into()).unwrap();
        assert!(tool.run(&["cat".into()]).is_ok());
        drop(tool);

        let tool_path = tools.get_tool_path("my_tool_path").unwrap();
        assert_eq!(tool_path.to_str().unwrap(), "./host_x64/my_tool_path");
        drop(tool_path);

        assert_eq!(&expected_log, tools.log());
    }
}
