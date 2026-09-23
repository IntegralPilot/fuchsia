// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod config;
mod driver;
mod env;
mod finder;
mod net;
mod runner;
mod yaml;

use crate::driver::infra::{InfraDriver, InfraDriverError};
use crate::runner::ExitStatus;

use std::fs;
use std::fs::File;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use argh::FromArgs;
use serde_yaml::Value;

#[derive(FromArgs)]
/// antlion runner with config generation
struct Args {
    /// name of the Fuchsia device to use for testing; defaults to using mDNS
    /// discovery
    #[argh(option)]
    device: Option<String>,

    /// path to the SSH binary used to communicate with all devices
    #[argh(option, from_str_fn(parse_file))]
    ssh_binary: PathBuf,

    /// path to the SSH private key used to communicate with Fuchsia; defaults
    /// to ~/.ssh/fuchsia_ed25519
    #[argh(option, from_str_fn(parse_file))]
    ssh_key: Option<PathBuf>,

    /// path to the FFX binary used to communicate with Fuchsia
    #[argh(option, from_str_fn(parse_file))]
    ffx_binary: PathBuf,

    /// search path to the FFX binary used to communicate with Fuchsia
    #[argh(option, from_str_fn(parse_directory))]
    ffx_subtools_search_path: Option<PathBuf>,

    /// path to the python interpreter binary (e.g. /bin/python3.9)
    #[argh(option)]
    python_bin: String,

    /// path to the antlion zipapp, ending in .pyz
    #[argh(option, from_str_fn(parse_file))]
    antlion_pyz: PathBuf,

    /// path to a directory for outputting artifacts; defaults to the current
    /// working directory or FUCHSIA_TEST_OUTDIR
    #[argh(option, from_str_fn(parse_directory))]
    out_dir: Option<PathBuf>,

    /// path to additional YAML config for this test; placed in the
    /// "test_params" key in the antlion config
    #[argh(option, from_str_fn(parse_file))]
    test_params: Option<PathBuf>,

    /// list of test cases to run; defaults to all test cases
    #[argh(positional)]
    test_cases: Vec<String>,

    /// user-defined configuration for the test; overrides all other options related to the test
    /// configratuion. By default, a config file will be generated based on the other parameters.
    #[argh(option, from_str_fn(parse_file))]
    config_override: Option<PathBuf>,

    /// ip of the AP
    #[argh(option)]
    ap_ip: Option<String>,

    /// ssh port of the AP
    #[argh(option)]
    ap_ssh_port: Option<u16>,

    /// path to the SSH private key used to communicate with the AP
    #[argh(option, from_str_fn(parse_file))]
    ap_ssh_key: Option<PathBuf>,
}

fn parse_file(s: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(s);
    let _ = File::open(&path).map_err(|e| format!("Failed to open \"{s}\": {e}"))?;
    Ok(path)
}

fn parse_directory(s: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(s);
    let meta =
        std::fs::metadata(&path).map_err(|e| format!("Failed to read metadata of \"{s}\": {e}"))?;
    if meta.is_file() {
        return Err(format!("Expected a directory but found a file at \"{s}\""));
    }
    Ok(path)
}

fn run_with_config<R>(runner: R, config_path: PathBuf) -> Result<ExitCode>
where
    R: runner::Runner,
{
    let exit_code = runner.run(config_path).context("Failed to run antlion")?;
    match exit_code {
        ExitStatus::Ok => println!("Antlion successfully exited"),
        ExitStatus::Err(code) => eprintln!("Antlion failed with status code {}", code),
        ExitStatus::Interrupt(Some(code)) => eprintln!("Antlion interrupted by signal {}", code),
        ExitStatus::Interrupt(None) => eprintln!("Antlion interrupted by signal"),
    };
    Ok(exit_code.into())
}

fn generate_config_and_run<R, D>(
    runner: R,
    driver: D,
    test_params: Option<Value>,
) -> Result<ExitCode>
where
    R: runner::Runner,
    D: driver::Driver,
{
    let mut config = driver.config();
    driver.setup().context("Failed to set up driver")?;

    if let Some(params) = test_params {
        config.merge_test_params(params);
    }

    let yaml =
        serde_yaml::to_string(&config).context("Failed to convert antlion config to YAML")?;

    let output_path = driver.output_path().to_path_buf();
    let config_path = output_path.join("config.yaml");
    println!("Generating config {}", config_path.display());
    println!("\n{yaml}\n");
    fs::write(&config_path, yaml).context("Failed to write config to file")?;

    let result = run_with_config(runner, config_path);
    driver.teardown().context("Failed to teardown environment")?;

    result
}

fn main() -> Result<ExitCode> {
    let args: Args = argh::from_env();
    let env = env::LocalEnvironment;
    let runner = runner::ProcessRunner {
        python_bin: args.python_bin,
        antlion_pyz: args.antlion_pyz,
        test_cases: args.test_cases,
    };

    let test_params = match args.test_params {
        Some(path) => {
            let text = fs::read_to_string(&path)
                .with_context(|| format!("Failed to read file \"{}\"", path.display()))?;
            let yaml = serde_yaml::from_str(&text)
                .with_context(|| format!("Failed to parse \"{text}\" as YAML"))?;
            Some(yaml)
        }
        None => None,
    };

    if let Some(config_path) = args.config_override {
        println!("Using config at {}", config_path.display());
        return run_with_config(runner, config_path);
    }

    match InfraDriver::new(
        env,
        args.ssh_binary.clone(),
        args.ffx_binary.clone(),
        args.ffx_subtools_search_path.clone(),
    ) {
        Ok(env) => return generate_config_and_run(runner, env, test_params),
        Err(InfraDriverError::NotDetected(_)) => {}
        Err(InfraDriverError::Config(e)) => {
            return Err(anyhow::Error::from(e).context("Config validation"));
        }
        Err(InfraDriverError::Other(e)) => {
            return Err(anyhow::Error::from(e).context("Unexpected infra driver error"));
        }
    };

    let ffx_finder = finder::FfxDevice { ffx_binary: args.ffx_binary.clone() };
    let driver_via_ffx_discovery = driver::local::LocalDriver::new(
        ffx_finder,
        args.device.clone(),
        args.ssh_binary.clone(),
        args.ssh_key.clone(),
        args.ffx_binary.clone(),
        args.ffx_subtools_search_path.clone(),
        args.out_dir.clone(),
        args.ap_ip.clone(),
        args.ap_ssh_port,
        args.ap_ssh_key.clone(),
    );
    match driver_via_ffx_discovery {
        Ok(driver) => return generate_config_and_run(runner, driver, test_params),
        Err(e) => {
            println!("Failed to generate device config via FFX: {:?}", e);
            println!("Falling back to mDNS discovery");
        }
    };

    let driver = driver::local::LocalDriver::new(
        finder::MulticastDns {},
        args.device.clone(),
        args.ssh_binary.clone(),
        args.ssh_key.clone(),
        args.ffx_binary.clone(),
        args.ffx_subtools_search_path.clone(),
        args.out_dir.clone(),
        args.ap_ip.clone(),
        args.ap_ssh_port,
        args.ap_ssh_key.clone(),
    )
    .context("Failed to generate config for local environment")?;
    generate_config_and_run(runner, driver, test_params)
}
