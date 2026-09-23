// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::target::GnTarget;
use anyhow::{anyhow, Result};
use std::path::PathBuf;
use std::process::Command;
use std::str::FromStr;

pub struct BuildScriptOutput {
    pub rustflags: Vec<String>,
    pub rustenv: Vec<String>,
}

impl BuildScriptOutput {
    pub fn parse_from_file(file: PathBuf) -> Result<Self> {
        let contents = std::fs::read_to_string(file)?;
        let configs: Vec<&str> = contents.split('\n').collect();
        Ok(Self::parse(configs))
    }

    pub fn parse(lines: Vec<&str>) -> Self {
        let mut bs = BuildScriptOutput { rustflags: vec![], rustenv: vec![] };

        for line in lines {
            if line.is_empty() {
                continue;
            }

            let keyval = line.strip_prefix("cargo:");
            if keyval.is_none() {
                eprintln!("warning: build script output includes non-cargo lines: {}", line);
                continue;
            }

            // More information on the available cargo instructions is here:
            // https://doc.rust-lang.org/cargo/reference/build-scripts.html#outputs-of-the-build-script
            //
            // The following cases are currently unsupported. They all have to do with specific
            // artifact type link arguments. The more general cargo:rustc-link-arg is covered.
            //
            // cargo:rustc-link-arg-bin=BIN=FLAG — Passes custom flags to a linker for the
            // binary BIN.
            //
            // cargo:rustc-link-arg-bins=FLAG — Passes custom flags to a linker for binaries.
            //
            // cargo:rustc-link-arg-tests=FLAG — Passes custom flags to a linker for tests.
            //
            // cargo:rustc-link-arg-examples=FLAG — Passes custom flags to a linker for
            // examples.
            //
            // cargo:rustc-link-arg-benches=FLAG — Passes custom flags to a linker for
            // benchmarks.
            //
            // cargo:rustc-cdylib-link-arg=FLAG — Passes custom flags to a linker for cdylib crates.

            let (key, value) = keyval.unwrap().split_once('=').unwrap();
            match key {
                "rerun-if-changed" => continue, // ignored because these are always vendored
                "rerun-if-env-changed" => continue, // ignored because these are always vendored
                "rustc-cfg" => bs.rustflags.push(format!("\"--cfg={}\"", value)),
                "rustc-env" => bs.rustenv.push(format!("\"{}\"", value)),
                "rustc-flags" => bs.rustflags.push(format!("\"{}\"", value)),
                "rustc-link-arg" => bs.rustflags.push(format!("\"-C link-arg={}\"", value)),
                "rustc-link-lib" => bs.rustflags.push(format!("\"-l {}\"", value)),
                // "rustc-link-search" => bs.rustflags.push(format!("-L {}", value)),
                "warning" => eprintln!("warning: {}", value),
                &_ => eprintln!("warning: unable to parse build script output: {}", value),
            }
        }

        bs
    }
}

pub struct BuildScript {}

impl BuildScript {
    pub fn execute(target: &GnTarget<'_>) -> Result<BuildScriptOutput> {
        // Previously, this code would compile build.rs scripts directly with rustc. However, build
        // scripts can have dependencies of their own (e.g. build-dependencies in Cargo.toml).
        // Rather than resolve the entire dependency chain ourselves, we call out to cargo to build
        // the crate, performing the build.rs compilation along the way. This does more work than
        // compiling the build.rs directly but we don't want to get into the dependency resolution
        // business when cargo can do this already.
        //
        // Unfortunately, there is no way to tell cargo to build only the build script. We could
        // link in the cargo library itself and call the correct functions to do only that part of
        // the build but that seems overkill.
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| std::ffi::OsString::from("cargo"));

        let command = Command::new(cargo)
            .current_dir(target.package_root())
            .arg("check")
            .arg("--message-format=json-render-diagnostics")
            .output()
            .expect("failed to execute cargo crate build");

        if !command.status.success() {
            return Err(anyhow!(
                "Failed to compile {}:\n{}",
                target.gn_target_name(),
                String::from_utf8_lossy(&command.stderr)
            ));
        }

        let stdout = String::from_utf8(command.stdout)?;
        let messages: Vec<&str> = stdout.trim().split('\n').collect();
        for message in messages {
            let json: serde_json::Value =
                serde_json::from_str(message).expect("JSON was not well-formatted");

            let reason = json.get("reason").unwrap();
            if reason != "build-script-executed" {
                continue;
            }

            let package_id = json.get("package_id").unwrap().to_string().replace('"', "");
            let package_elements: Vec<&str> = package_id.split(' ').collect();
            let crate_name = package_elements[0];

            // this build script execution was for the crate that we are expecting to have a
            // build script (avoids issues where a dependency has a build script and we use
            // that one by mistake)
            if crate_name != target.target_name {
                continue;
            }

            // cargo executes the build script for us and stores the output in its cache, we just
            // need to parse it
            let out_dir = json.get("out_dir").unwrap().as_str().unwrap();
            let output = PathBuf::from_str(out_dir).unwrap().parent().unwrap().join("output");
            return BuildScriptOutput::parse_from_file(output);
        }

        Err(anyhow!(
            "We detected a build script in {} but cargo didn't compile it",
            target.gn_target_name()
        ))
    }
}
