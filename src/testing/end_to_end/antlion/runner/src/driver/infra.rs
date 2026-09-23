// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::config::{self, Config, PduRef};
use crate::driver::Driver;
use crate::env::Environment;
use crate::net::IpAddr;
use crate::yaml;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use itertools::Itertools;
use serde::Deserialize;
use serde_yaml::Value;
use thiserror::Error;

const TESTBED_NAME: &'static str = "antlion-runner";
const ENV_OUT_DIR: &'static str = "FUCHSIA_TEST_OUTDIR";
const ENV_TESTBED_CONFIG: &'static str = "FUCHSIA_TESTBED_CONFIG";
const TEST_SUMMARY_FILE: &'static str = "test_summary.yaml";

#[derive(Debug)]
/// Driver for running antlion on emulated and hardware testbeds hosted by
/// Fuchsia infrastructure.
pub(crate) struct InfraDriver {
    output_dir: PathBuf,
    config: Config,
}

#[derive(Error, Debug)]
pub(crate) enum InfraDriverError {
    #[error("infra environment not detected, \"{0}\" environment variable not present")]
    NotDetected(String),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[derive(Error, Debug)]
pub(crate) enum ConfigError {
    #[error("ip {ip} in use by several devices")]
    DuplicateIp { ip: IpAddr },
    #[error("ip {ip} port {port} in use by several devices")]
    DuplicatePort { ip: IpAddr, port: u8 },
}

impl InfraDriver {
    /// Detect an InfraDriver. Returns None if the required environmental
    /// variables are not found.
    pub fn new<E: Environment>(
        env: E,
        ssh_binary: PathBuf,
        ffx_binary: PathBuf,
        ffx_subtools_search_path: Option<PathBuf>,
    ) -> Result<Self, InfraDriverError> {
        let config_path = match env.var(ENV_TESTBED_CONFIG) {
            Ok(p) => PathBuf::from(p),
            Err(std::env::VarError::NotPresent) => {
                return Err(InfraDriverError::NotDetected(ENV_TESTBED_CONFIG.to_string()));
            }
            Err(e) => {
                return Err(InfraDriverError::Other(anyhow!(
                    "Failed to read \"{ENV_TESTBED_CONFIG}\" {e}"
                )));
            }
        };
        let config = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read \"{}\"", config_path.display()))?;
        let targets: Vec<InfraTarget> = serde_json::from_str(&config)
            .with_context(|| format!("Failed to parse into InfraTarget: \"{config}\""))?;
        if targets.len() == 0 {
            return Err(InfraDriverError::Other(anyhow!(
                "Expected at least one target declared in \"{}\"",
                config_path.display()
            )));
        }

        let output_path = match env.var(ENV_OUT_DIR) {
            Ok(p) => p,
            Err(std::env::VarError::NotPresent) => {
                return Err(InfraDriverError::NotDetected(ENV_OUT_DIR.to_string()));
            }
            Err(e) => {
                return Err(InfraDriverError::Other(anyhow!(
                    "Failed to read \"{ENV_OUT_DIR}\" {e}"
                )));
            }
        };
        let output_dir = PathBuf::from(output_path);
        if !fs::metadata(&output_dir).context("Failed to stat the output directory")?.is_dir() {
            return Err(InfraDriverError::Other(anyhow!(
                "Expected a directory but found a file at \"{}\"",
                output_dir.display()
            )));
        }

        Ok(InfraDriver {
            output_dir: output_dir.clone(),
            config: InfraDriver::parse_targets(
                targets,
                ssh_binary,
                ffx_binary,
                ffx_subtools_search_path,
                output_dir,
            )?,
        })
    }

    fn parse_targets(
        targets: Vec<InfraTarget>,
        ssh_binary: PathBuf,
        ffx_binary: PathBuf,
        ffx_subtools_search_path: Option<PathBuf>,
        output_dir: PathBuf,
    ) -> Result<Config, InfraDriverError> {
        let mut fuchsia_devices: Vec<config::Fuchsia> = vec![];
        let mut access_points: Vec<config::AccessPoint> = vec![];
        let mut openwrt_aps: Vec<config::AccessPoint> = vec![];
        let mut attenuators: HashMap<IpAddr, config::Attenuator> = HashMap::new();
        let mut pdus: HashMap<IpAddr, config::Pdu> = HashMap::new();
        let mut iperf_servers: Vec<config::IPerfServer> = vec![];
        let mut test_params: Option<Value> = None;

        let mut used_ips: HashSet<IpAddr> = HashSet::new();
        let mut used_ports: HashMap<IpAddr, HashSet<u8>> = HashMap::new();

        let mut register_ip = |ip: IpAddr| -> Result<(), InfraDriverError> {
            if !used_ips.insert(ip.clone()) {
                return Err(ConfigError::DuplicateIp { ip }.into());
            }
            Ok(())
        };

        let mut register_port = |ip: IpAddr, port: u8| -> Result<(), InfraDriverError> {
            match used_ports.get_mut(&ip) {
                Some(ports) => {
                    if !ports.insert(port) {
                        return Err(ConfigError::DuplicatePort { ip, port }.into());
                    }
                }
                None => {
                    if used_ports.insert(ip, HashSet::from([port])).is_some() {
                        return Err(InfraDriverError::Other(anyhow!(
                            "Used ports set was unexpectedly modified by concurrent use",
                        )));
                    }
                }
            };
            Ok(())
        };

        let mut register_pdu = |p: Option<PduRef>| -> Result<(), InfraDriverError> {
            if let Some(PduRef { device, ip, port }) = p {
                register_port(ip.clone(), port)?;
                let new = config::Pdu { device, host: ip.clone() };
                if let Some(old) = pdus.insert(ip.clone(), new.clone()) {
                    if old != new {
                        return Err(ConfigError::DuplicateIp { ip }.into());
                    }
                }
            }
            Ok(())
        };

        let mut register_attenuator = |a: Option<AttenuatorRef>| -> Result<(), InfraDriverError> {
            if let Some(a) = a {
                let new = config::Attenuator {
                    model: "minicircuits".to_string(),
                    instrument_count: 4,
                    address: a.ip.clone(),
                    protocol: "http".to_string(),
                    port: 80,
                };
                if let Some(old) = attenuators.insert(a.ip.clone(), new.clone()) {
                    if old != new {
                        return Err(ConfigError::DuplicateIp { ip: a.ip }.into());
                    }
                }
            }
            Ok(())
        };

        let mut merge_test_params = |p: Option<Value>| {
            match (test_params.as_mut(), p) {
                (None, Some(new)) => test_params = Some(new),
                (Some(existing), Some(new)) => yaml::merge(existing, new),
                (_, None) => {}
            };
        };

        for target in targets {
            match target {
                InfraTarget::FuchsiaDevice { nodename, ipv4, ipv6, ssh_key, pdu, test_params } => {
                    let ip: IpAddr = if !ipv4.is_empty() {
                        ipv4.parse().context("Invalid IPv4 address")
                    } else if !ipv6.is_empty() {
                        ipv6.parse().context("Invalid IPv6 address")
                    } else {
                        Err(anyhow!("IP address not specified"))
                    }?;

                    fuchsia_devices.push(config::Fuchsia {
                        name: nodename,
                        ip: ip.clone(),
                        device_ip_port: format!("{}", ip), // If ssh_port is ever set, this needs to be updated
                        ssh_port: None,
                        take_bug_report_on_fail: true,
                        ssh_binary_path: ssh_binary.clone(),
                        // TODO(http://b/244747218): Remove when ssh_config is refactored away
                        ssh_config: None,
                        ffx_binary_path: ffx_binary.clone(),
                        ffx_subtools_search_path: ffx_subtools_search_path.clone(),
                        ssh_priv_key: ssh_key,
                        pdu_device: pdu.clone(),
                        hard_reboot_on_fail: true,
                        honeydew_config: config::HoneydewConfig {
                            transports: config::HoneydewTransports {
                                ffx: config::HoneydewFfx {
                                    path: ffx_binary.clone(),
                                    subtools_search_path: ffx_subtools_search_path.clone(),
                                },
                            },
                            affordances: config::HoneydewAffordances {
                                bluetooth: config::HoneydewAffordanceSpec {
                                    implementation: config::HONEYDEW_IMPL_FUCHSIA_CONTROLLER
                                        .to_string(),
                                },
                            },
                        },
                    });

                    register_ip(ip)?;
                    register_pdu(pdu)?;
                    merge_test_params(test_params);
                }
                InfraTarget::AccessPoint { ip, nodename, model, attenuator, pdu, ssh_key } => {
                    let ap = config::AccessPoint {
                        wan_interface: "eth0".to_string(),
                        nodename,
                        ssh_config: config::SshConfig {
                            ssh_binary_path: ssh_binary.clone(),
                            host: ip.clone(),
                            port: None,
                            user: "root".to_string(),
                            identity_file: ssh_key,
                        },
                        pdu_device: pdu.clone(),
                        attenuators: attenuator.as_ref().map(|a| {
                            vec![config::AttenuatorRef {
                                address: a.ip.clone(),
                                ports_2g: vec![1, 2, 3],
                                ports_5g: vec![1, 2, 3],
                            }]
                        }),
                        allow_regdb_bypass: Some(true),
                    };

                    if let Some(m) = model.as_deref() {
                        if m == "OpenWrtOne" {
                            openwrt_aps.push(ap);
                        } else {
                            access_points.push(ap);
                        }
                    } else {
                        access_points.push(ap);
                    };

                    register_ip(ip)?;
                    register_pdu(pdu)?;
                    register_attenuator(attenuator)?;
                }
                InfraTarget::IPerfServer { ip, user, test_interface, pdu, ssh_key } => {
                    iperf_servers.push(config::IPerfServer {
                        ssh_config: config::SshConfig {
                            ssh_binary_path: ssh_binary.clone(),
                            host: ip.clone(),
                            port: None,
                            user,
                            identity_file: ssh_key,
                        },
                        port: 5201,
                        test_interface: test_interface,
                        use_killall: true,
                    });

                    register_ip(ip)?;
                    register_pdu(pdu)?;
                }
            };
        }

        Ok(Config {
            testbeds: vec![config::Testbed {
                name: TESTBED_NAME.to_string(),
                controllers: config::Controllers {
                    fuchsia_devices: fuchsia_devices,
                    access_points: access_points,
                    openwrt_aps: openwrt_aps,
                    attenuators: attenuators
                        .into_values()
                        .sorted_by_key(|a| a.address.clone())
                        .collect(),
                    pdus: pdus.into_values().sorted_by_key(|p| p.host.clone()).collect(),
                    iperf_servers: iperf_servers,
                },
                test_params,
            }],
            mobly_params: config::MoblyParams { log_path: output_dir },
        })
    }
}

impl Driver for InfraDriver {
    fn output_path(&self) -> &Path {
        self.output_dir.as_path()
    }
    fn config(&self) -> Config {
        self.config.clone()
    }
    fn setup(&self) -> Result<()> {
        // This string matches //tools/testing/testparser/testparser.go, to indicate that Mobly
        // results will be printed in the test
        // LINT.IfChange(mobly_test_start)
        println!("======== Mobly config content ========");
        // LINT.ThenChange(//src/testing/end_to_end/mobly_driver/mobly_driver/api/api_infra.py:mobly_test_start)
        Ok(())
    }
    fn teardown(&self) -> Result<()> {
        let results_path =
            self.output_dir.join(TESTBED_NAME).join("latest").join(TEST_SUMMARY_FILE);
        match fs::File::open(&results_path) {
            Ok(mut results) => {
                println!("\nTest results from {}\n", results_path.display());
                // This string matched //tools/testing/testparser/moblytest.go, so the test parser
                // knows when test results start.
                // LINT.IfChange(mobly_test_end)
                println!("[=====MOBLY RESULTS=====]");
                // LINT.ThenChange(//src/testing/end_to_end/mobly_driver/mobly_driver/api/api_infra.py:mobly_test_end)
                std::io::copy(&mut results, &mut std::io::stdout())
                    .context("Failed to copy results to stdout")?;
            }
            Err(e) => eprintln!("Failed to open \"{}\": {}", results_path.display(), e),
        };

        // Remove any symlinks from the output directory; this causes errors
        // while uploading to CAS.
        //
        // TODO: Remove when the fix is released and supported on Swarming bots
        // https://github.com/bazelbuild/remote-apis-sdks/pull/229.
        remove_symlinks(self.output_dir.clone())?;

        Ok(())
    }
}

fn remove_symlinks<P: AsRef<Path>>(path: P) -> Result<()> {
    let meta = fs::symlink_metadata(path.as_ref())?;
    if meta.is_symlink() {
        fs::remove_file(path)?;
    } else if meta.is_dir() {
        for entry in fs::read_dir(path)? {
            remove_symlinks(entry?.path())?;
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
/// Schema used to communicate target information from the test environment set
/// up by botanist.
///
/// See https://cs.opensource.google/fuchsia/fuchsia/+/main:tools/botanist/README.md
enum InfraTarget {
    FuchsiaDevice {
        nodename: String,
        ipv4: String,
        ipv6: String,
        ssh_key: PathBuf,
        pdu: Option<PduRef>,
        test_params: Option<Value>,
    },
    AccessPoint {
        ip: IpAddr,
        nodename: Option<String>,
        model: Option<String>,
        ssh_key: PathBuf,
        attenuator: Option<AttenuatorRef>,
        pdu: Option<PduRef>,
    },
    IPerfServer {
        ip: IpAddr,
        ssh_key: PathBuf,
        #[serde(default = "default_iperf_user")]
        user: String,
        test_interface: String,
        pdu: Option<PduRef>,
    },
}

fn default_iperf_user() -> String {
    "pi".to_string()
}

#[derive(Clone, Debug, Deserialize)]
struct AttenuatorRef {
    ip: IpAddr,
}

#[cfg(test)]
mod test {
    use super::*;

    use crate::generate_config_and_run;
    use crate::runner::{ExitStatus, Runner};

    use std::ffi::OsStr;

    use assert_matches::assert_matches;
    use indoc::formatdoc;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use tempfile::{NamedTempFile, TempDir};

    const FUCHSIA_NAME: &'static str = "fuchsia-1234-5678-9abc";
    const FUCHSIA_ADDR: &'static str = "fe80::1%2";

    #[derive(Default)]
    struct MockRunner {
        out_dir: PathBuf,
        config: std::cell::Cell<PathBuf>,
    }
    impl MockRunner {
        fn new(out_dir: PathBuf) -> Self {
            Self { out_dir, ..Default::default() }
        }
    }
    impl Runner for MockRunner {
        fn run(&self, config: PathBuf) -> Result<ExitStatus> {
            self.config.set(config);

            let antlion_out = self.out_dir.join(TESTBED_NAME).join("latest");
            fs::create_dir_all(&antlion_out)
                .context("Failed to create antlion output directory")?;
            fs::write(antlion_out.join(TEST_SUMMARY_FILE), "")
                .context("Failed to write test_summary.yaml")?;
            Ok(ExitStatus::Ok)
        }
    }

    struct MockEnvironment {
        config: Option<PathBuf>,
        out_dir: Option<PathBuf>,
    }
    impl Environment for MockEnvironment {
        fn var<K: AsRef<OsStr>>(&self, key: K) -> Result<String, std::env::VarError> {
            if key.as_ref() == ENV_TESTBED_CONFIG {
                self.config
                    .clone()
                    .ok_or(std::env::VarError::NotPresent)
                    .map(|p| p.into_os_string().into_string().unwrap())
            } else if key.as_ref() == ENV_OUT_DIR {
                self.out_dir
                    .clone()
                    .ok_or(std::env::VarError::NotPresent)
                    .map(|p| p.into_os_string().into_string().unwrap())
            } else {
                Err(std::env::VarError::NotPresent)
            }
        }
    }

    #[test]
    fn infra_not_detected() {
        let ssh = NamedTempFile::new().unwrap();
        let ffx = NamedTempFile::new().unwrap();
        let env = MockEnvironment { config: None, out_dir: None };

        let got = InfraDriver::new(env, ssh.path().to_path_buf(), ffx.path().to_path_buf(), None);
        assert_matches!(got, Err(InfraDriverError::NotDetected(_)));
    }

    #[test]
    fn infra_not_detected_config() {
        let ssh = NamedTempFile::new().unwrap();
        let ffx = NamedTempFile::new().unwrap();
        let out_dir = TempDir::new().unwrap();
        let env = MockEnvironment { config: None, out_dir: Some(out_dir.path().to_path_buf()) };

        let got = InfraDriver::new(env, ssh.path().to_path_buf(), ffx.path().to_path_buf(), None);
        assert_matches!(got, Err(InfraDriverError::NotDetected(v)) if v == ENV_TESTBED_CONFIG);
    }

    #[test]
    fn infra_not_detected_out_dir() {
        let ssh = NamedTempFile::new().unwrap();
        let ssh_key = NamedTempFile::new().unwrap();
        let ffx = NamedTempFile::new().unwrap();

        let testbed_config = NamedTempFile::new().unwrap();
        serde_json::to_writer_pretty(
            testbed_config.as_file(),
            &json!([{
                "type": "FuchsiaDevice",
                "nodename": FUCHSIA_NAME,
                "ipv4": "",
                "ipv6": FUCHSIA_ADDR,
                "ssh_key": ssh_key.path(),
            }]),
        )
        .unwrap();

        let env =
            MockEnvironment { config: Some(testbed_config.path().to_path_buf()), out_dir: None };

        let got = InfraDriver::new(env, ssh.path().to_path_buf(), ffx.path().to_path_buf(), None);
        assert_matches!(got, Err(InfraDriverError::NotDetected(v)) if v == ENV_OUT_DIR);
    }

    #[test]
    fn infra_invalid_config() {
        let ssh = NamedTempFile::new().unwrap();
        let ffx = NamedTempFile::new().unwrap();
        let out_dir = TempDir::new().unwrap();

        let testbed_config = NamedTempFile::new().unwrap();
        serde_json::to_writer_pretty(testbed_config.as_file(), &json!({ "foo": "bar" })).unwrap();

        let env = MockEnvironment {
            config: Some(testbed_config.path().to_path_buf()),
            out_dir: Some(out_dir.path().to_path_buf()),
        };

        let got = InfraDriver::new(env, ssh.path().to_path_buf(), ffx.path().to_path_buf(), None);
        assert_matches!(got, Err(_));
    }

    #[test]
    fn infra() {
        let ssh = NamedTempFile::new().unwrap();
        let ssh_key = NamedTempFile::new().unwrap();
        let ffx_subtools = TempDir::new().unwrap();
        let ffx = NamedTempFile::new().unwrap();
        let out_dir = TempDir::new().unwrap();

        let testbed_config = NamedTempFile::new().unwrap();
        serde_json::to_writer_pretty(
            testbed_config.as_file(),
            &json!([{
                "type": "FuchsiaDevice",
                "nodename": FUCHSIA_NAME,
                "ipv4": "",
                "ipv6": FUCHSIA_ADDR,
                "ssh_key": ssh_key.path(),
            }]),
        )
        .unwrap();

        let runner = MockRunner::new(out_dir.path().to_path_buf());
        let env = MockEnvironment {
            config: Some(testbed_config.path().to_path_buf()),
            out_dir: Some(out_dir.path().to_path_buf()),
        };
        let driver = InfraDriver::new(
            env,
            ssh.path().to_path_buf(),
            ffx.path().to_path_buf(),
            Some(ffx_subtools.path().to_path_buf()),
        )
        .unwrap();
        generate_config_and_run(runner, driver, None).unwrap();

        let got = fs::read_to_string(out_dir.path().join("config.yaml")).unwrap();

        let ssh_path = ssh.path().display().to_string();
        let ssh_key_path = ssh_key.path().display().to_string();
        let ffx_path = ffx.path().display().to_string();
        let ffx_subtools_path = ffx_subtools.path().display();
        let out_path = out_dir.path().display();
        let want = formatdoc! {r#"
        TestBeds:
        - Name: {TESTBED_NAME}
          Controllers:
            FuchsiaDevice:
            - name: {FUCHSIA_NAME}
              ip: {FUCHSIA_ADDR}
              device_ip_port: {FUCHSIA_ADDR}
              take_bug_report_on_fail: true
              ssh_binary_path: {ssh_path}
              ffx_binary_path: {ffx_path}
              ffx_subtools_search_path: {ffx_subtools_path}
              ssh_priv_key: {ssh_key_path}
              hard_reboot_on_fail: true
              honeydew_config:
                transports:
                  ffx:
                    path: {ffx_path}
                    subtools_search_path: {ffx_subtools_path}
                affordances:
                  bluetooth:
                    implementation: fuchsia-controller
        MoblyParams:
          LogPath: {out_path}
        "#};

        assert_eq!(got, want);
    }

    #[test]
    fn infra_with_test_params() {
        let ssh = NamedTempFile::new().unwrap();
        let ssh_key = NamedTempFile::new().unwrap();
        let ffx = NamedTempFile::new().unwrap();
        let ffx_subtools = TempDir::new().unwrap();
        let out_dir = TempDir::new().unwrap();

        let testbed_config = NamedTempFile::new().unwrap();
        serde_json::to_writer_pretty(
            testbed_config.as_file(),
            &json!([{
                "type": "FuchsiaDevice",
                "nodename": FUCHSIA_NAME,
                "ipv4": "",
                "ipv6": FUCHSIA_ADDR,
                "ssh_key": ssh_key.path(),
                "test_params": {
                    "my_test_params": {
                        "can_overwrite": false,
                        "from_original": true,
                    }
                }
            }]),
        )
        .unwrap();

        let runner = MockRunner::new(out_dir.path().to_path_buf());
        let env = MockEnvironment {
            config: Some(testbed_config.path().to_path_buf()),
            out_dir: Some(out_dir.path().to_path_buf()),
        };
        let driver = InfraDriver::new(
            env,
            ssh.path().to_path_buf(),
            ffx.path().to_path_buf(),
            Some(ffx_subtools.path().to_path_buf()),
        )
        .unwrap();
        let params = "
            my_test_params:
                merged_with: true
                can_overwrite: true
        ";
        let params = serde_yaml::from_str(params).unwrap();
        generate_config_and_run(runner, driver, Some(params)).unwrap();

        let got = fs::read_to_string(out_dir.path().join("config.yaml")).unwrap();

        let ssh_path = ssh.path().display().to_string();
        let ssh_key_path = ssh_key.path().display().to_string();
        let ffx_path = ffx.path().display().to_string();
        let ffx_subtools_path = ffx_subtools.path().display();
        let out_path = out_dir.path().display();
        let want = formatdoc! {r#"
        TestBeds:
        - Name: {TESTBED_NAME}
          Controllers:
            FuchsiaDevice:
            - name: {FUCHSIA_NAME}
              ip: {FUCHSIA_ADDR}
              device_ip_port: {FUCHSIA_ADDR}
              take_bug_report_on_fail: true
              ssh_binary_path: {ssh_path}
              ffx_binary_path: {ffx_path}
              ffx_subtools_search_path: {ffx_subtools_path}
              ssh_priv_key: {ssh_key_path}
              hard_reboot_on_fail: true
              honeydew_config:
                transports:
                  ffx:
                    path: {ffx_path}
                    subtools_search_path: {ffx_subtools_path}
                affordances:
                  bluetooth:
                    implementation: fuchsia-controller
          TestParams:
            my_test_params:
              can_overwrite: true
              from_original: true
              merged_with: true
        MoblyParams:
          LogPath: {out_path}
        "#};

        assert_eq!(got, want);
    }

    #[test]
    fn infra_with_auxiliary_devices() {
        const FUCHSIA_PDU_IP: &'static str = "192.168.42.14";
        const FUCHSIA_PDU_PORT: u8 = 1;
        const AP_IP: &'static str = "192.168.42.11";
        const AP_NODENAME: &'static str = "access-point-1234-5678-9abc";
        const AP_AND_IPERF_PDU_IP: &'static str = "192.168.42.13";
        const AP_PDU_PORT: u8 = 1;
        const ATTENUATOR_IP: &'static str = "192.168.42.15";
        const IPERF_IP: &'static str = "192.168.42.12";
        const IPERF_USER: &'static str = "alice";
        const IPERF_PDU_PORT: u8 = 2;

        let ssh = NamedTempFile::new().unwrap();
        let ssh_key = NamedTempFile::new().unwrap();
        let ffx = NamedTempFile::new().unwrap();
        let ffx_subtools = TempDir::new().unwrap();
        let out_dir = TempDir::new().unwrap();

        let testbed_config = NamedTempFile::new().unwrap();
        serde_json::to_writer_pretty(
            testbed_config.as_file(),
            &json!([{
                "type": "FuchsiaDevice",
                "nodename": FUCHSIA_NAME,
                "ipv4": "",
                "ipv6": FUCHSIA_ADDR,
                "ssh_key": ssh_key.path(),
                "pdu": {
                    "ip": FUCHSIA_PDU_IP,
                    "port": FUCHSIA_PDU_PORT,
                },
            }, {
                "type": "AccessPoint",
                "ip": AP_IP,
                "nodename": AP_NODENAME,
                "ssh_key": ssh_key.path(),
                "attenuator": {
                    "ip": ATTENUATOR_IP,
                },
                "pdu": {
                    "ip": AP_AND_IPERF_PDU_IP,
                    "port": AP_PDU_PORT,
                    "device": "fancy-pdu",
                },
            }, {
                "type": "IPerfServer",
                "ip": IPERF_IP,
                "ssh_key": ssh_key.path(),
                "user": IPERF_USER,
                "test_interface": "eth0",
                "pdu": {
                    "ip": AP_AND_IPERF_PDU_IP,
                    "port": IPERF_PDU_PORT,
                    "device": "fancy-pdu",
                },
            }]),
        )
        .unwrap();

        let runner = MockRunner::new(out_dir.path().to_path_buf());
        let env = MockEnvironment {
            config: Some(testbed_config.path().to_path_buf()),
            out_dir: Some(out_dir.path().to_path_buf()),
        };
        let driver = InfraDriver::new(
            env,
            ssh.path().to_path_buf(),
            ffx.path().to_path_buf(),
            Some(ffx_subtools.path().to_path_buf()),
        )
        .unwrap();
        generate_config_and_run(runner, driver, None).unwrap();

        let got = std::fs::read_to_string(out_dir.path().join("config.yaml")).unwrap();

        let ssh_path = ssh.path().display().to_string();
        let ssh_key_path = ssh_key.path().display().to_string();
        let ffx_path = ffx.path().display().to_string();
        let ffx_subtools_path = ffx_subtools.path().display();
        let out_path = out_dir.path().display();
        let want = formatdoc! {r#"
        TestBeds:
        - Name: {TESTBED_NAME}
          Controllers:
            FuchsiaDevice:
            - name: {FUCHSIA_NAME}
              ip: {FUCHSIA_ADDR}
              device_ip_port: {FUCHSIA_ADDR}
              take_bug_report_on_fail: true
              ssh_binary_path: {ssh_path}
              ffx_binary_path: {ffx_path}
              ffx_subtools_search_path: {ffx_subtools_path}
              ssh_priv_key: {ssh_key_path}
              PduDevice:
                device: synaccess.np02b
                host: {FUCHSIA_PDU_IP}
                port: {FUCHSIA_PDU_PORT}
              hard_reboot_on_fail: true
              honeydew_config:
                transports:
                  ffx:
                    path: {ffx_path}
                    subtools_search_path: {ffx_subtools_path}
                affordances:
                  bluetooth:
                    implementation: fuchsia-controller
            AccessPoint:
            - wan_interface: eth0
              nodename: {AP_NODENAME}
              ssh_config:
                ssh_binary_path: {ssh_path}
                host: {AP_IP}
                user: root
                identity_file: {ssh_key_path}
              PduDevice:
                device: fancy-pdu
                host: {AP_AND_IPERF_PDU_IP}
                port: {AP_PDU_PORT}
              Attenuator:
              - Address: {ATTENUATOR_IP}
                attenuator_ports_wifi_2g:
                - 1
                - 2
                - 3
                attenuator_ports_wifi_5g:
                - 1
                - 2
                - 3
              allow_regdb_bypass: true
            Attenuator:
            - Model: minicircuits
              InstrumentCount: 4
              Address: {ATTENUATOR_IP}
              Protocol: http
              Port: 80
            PduDevice:
            - device: fancy-pdu
              host: {AP_AND_IPERF_PDU_IP}
            - device: synaccess.np02b
              host: {FUCHSIA_PDU_IP}
            IPerfServer:
            - ssh_config:
                ssh_binary_path: {ssh_path}
                host: {IPERF_IP}
                user: {IPERF_USER}
                identity_file: {ssh_key_path}
              port: 5201
              test_interface: eth0
              use_killall: true
        MoblyParams:
          LogPath: {out_path}
        "#};

        assert_eq!(got, want);
    }
    #[test]
    fn infra_with_openwrt_ap() {
        let ssh = NamedTempFile::new().unwrap();
        let ssh_key = NamedTempFile::new().unwrap();
        let ffx_subtools = TempDir::new().unwrap();
        let ffx = NamedTempFile::new().unwrap();
        let out_dir = TempDir::new().unwrap();

        let testbed_config = NamedTempFile::new().unwrap();
        serde_json::to_writer_pretty(
            testbed_config.as_file(),
            &json!([{
                "type": "FuchsiaDevice",
                "nodename": FUCHSIA_NAME,
                "ipv4": "",
                "ipv6": FUCHSIA_ADDR,
                "ssh_key": ssh_key.path(),
            }, {
                "type": "AccessPoint",
                "ip": "192.168.42.11",
                "ssh_key": ssh_key.path(),
                "model": "OpenWrtOne",
            }]),
        )
        .unwrap();

        let runner = MockRunner::new(out_dir.path().to_path_buf());
        let env = MockEnvironment {
            config: Some(testbed_config.path().to_path_buf()),
            out_dir: Some(out_dir.path().to_path_buf()),
        };
        let driver = InfraDriver::new(
            env,
            ssh.path().to_path_buf(),
            ffx.path().to_path_buf(),
            Some(ffx_subtools.path().to_path_buf()),
        )
        .unwrap();
        generate_config_and_run(runner, driver, None).unwrap();

        let got = std::fs::read_to_string(out_dir.path().join("config.yaml")).unwrap();

        let ssh_path = ssh.path().display().to_string();
        let ssh_key_path = ssh_key.path().display().to_string();
        let ffx_path = ffx.path().display().to_string();
        let ffx_subtools_path = ffx_subtools.path().display();
        let out_path = out_dir.path().display();
        let want = formatdoc! {r#"
        TestBeds:
        - Name: {TESTBED_NAME}
          Controllers:
            FuchsiaDevice:
            - name: {FUCHSIA_NAME}
              ip: {FUCHSIA_ADDR}
              device_ip_port: {FUCHSIA_ADDR}
              take_bug_report_on_fail: true
              ssh_binary_path: {ssh_path}
              ffx_binary_path: {ffx_path}
              ffx_subtools_search_path: {ffx_subtools_path}
              ssh_priv_key: {ssh_key_path}
              hard_reboot_on_fail: true
              honeydew_config:
                transports:
                  ffx:
                    path: {ffx_path}
                    subtools_search_path: {ffx_subtools_path}
                affordances:
                  bluetooth:
                    implementation: fuchsia-controller
            OpenWrtAP:
            - wan_interface: eth0
              ssh_config:
                ssh_binary_path: {ssh_path}
                host: 192.168.42.11
                user: root
                identity_file: {ssh_key_path}
              allow_regdb_bypass: true
        MoblyParams:
          LogPath: {out_path}
        "#};

        assert_eq!(got, want);
    }

    #[test]
    fn infra_duplicate_port_pdu() {
        let pdu_ip: IpAddr = "192.168.42.13".parse().unwrap();
        let pdu_port = 1;

        let ssh = NamedTempFile::new().unwrap();
        let ssh_key = NamedTempFile::new().unwrap();
        let ffx = NamedTempFile::new().unwrap();
        let out_dir = TempDir::new().unwrap();

        let testbed_config = NamedTempFile::new().unwrap();
        serde_json::to_writer_pretty(
            testbed_config.as_file(),
            &json!([{
                "type": "FuchsiaDevice",
                "nodename": "foo",
                "ipv4": "",
                "ipv6": "fe80::1%2",
                "ssh_key": ssh_key.path(),
                "pdu": {
                    "ip": pdu_ip,
                    "port": pdu_port,
                },
            }, {
                "type": "AccessPoint",
                "ip": "192.168.42.11",
                "ssh_key": ssh_key.path(),
                "pdu": {
                    "ip": pdu_ip,
                    "port": pdu_port,
                },
            }]),
        )
        .unwrap();

        let env = MockEnvironment {
            config: Some(testbed_config.path().to_path_buf()),
            out_dir: Some(out_dir.path().to_path_buf()),
        };
        let got = InfraDriver::new(env, ssh.path().to_path_buf(), ffx.path().to_path_buf(), None);
        assert_matches!(got,
            Err(InfraDriverError::Config(ConfigError::DuplicatePort { ip, port }))
                if ip == pdu_ip && port == pdu_port
        );
    }

    #[test]
    fn infra_duplicate_ip_pdu() {
        let duplicate_ip: IpAddr = "192.168.42.13".parse().unwrap();

        let ssh = NamedTempFile::new().unwrap();
        let ssh_key = NamedTempFile::new().unwrap();
        let ffx = NamedTempFile::new().unwrap();
        let out_dir = TempDir::new().unwrap();

        let testbed_config = NamedTempFile::new().unwrap();
        serde_json::to_writer_pretty(
            testbed_config.as_file(),
            &json!([{
                "type": "FuchsiaDevice",
                "nodename": "foo",
                "ipv4": "",
                "ipv6": "fe80::1%2",
                "ssh_key": ssh_key.path(),
                "pdu": {
                    "ip": duplicate_ip,
                    "port": 1,
                    "device": "A",
                },
            }, {
                "type": "AccessPoint",
                "ip": "192.168.42.11",
                "ssh_key": ssh_key.path(),
                "pdu": {
                    "ip": duplicate_ip,
                    "port": 2,
                    "device": "B",
                },
            }]),
        )
        .unwrap();

        let env = MockEnvironment {
            config: Some(testbed_config.path().to_path_buf()),
            out_dir: Some(out_dir.path().to_path_buf()),
        };
        assert_matches!(
            InfraDriver::new(env, ssh.path().to_path_buf(), ffx.path().to_path_buf(), None),
            Err(InfraDriverError::Config(ConfigError::DuplicateIp { ip }))
                if ip == duplicate_ip
        );
    }

    #[test]
    fn infra_duplicate_ip_devices() {
        let duplicate_ip: IpAddr = "192.168.42.11".parse().unwrap();

        let ssh = NamedTempFile::new().unwrap();
        let ssh_key = NamedTempFile::new().unwrap();
        let ffx = NamedTempFile::new().unwrap();
        let out_dir = TempDir::new().unwrap();

        let testbed_config = NamedTempFile::new().unwrap();
        serde_json::to_writer_pretty(
            testbed_config.as_file(),
            &json!([{
                "type": "FuchsiaDevice",
                "nodename": "foo",
                "ipv4": duplicate_ip,
                "ipv6": "",
                "ssh_key": ssh_key.path(),
            }, {
                "type": "AccessPoint",
                "ip": duplicate_ip,
                "ssh_key": ssh_key.path(),
            }]),
        )
        .unwrap();

        let env = MockEnvironment {
            config: Some(testbed_config.path().to_path_buf()),
            out_dir: Some(out_dir.path().to_path_buf()),
        };
        let got = InfraDriver::new(env, ssh.path().to_path_buf(), ffx.path().to_path_buf(), None);
        assert_matches!(got,
            Err(InfraDriverError::Config(ConfigError::DuplicateIp { ip }))
                if ip == duplicate_ip
        );
    }

    #[test]
    fn remove_symlinks_works() {
        const SYMLINK_FILE: &'static str = "latest";

        let out_dir = TempDir::new().unwrap();
        let test_file = NamedTempFile::new_in(&out_dir).unwrap();
        let symlink_path = out_dir.path().join(SYMLINK_FILE);

        #[cfg(unix)]
        std::os::unix::fs::symlink(&test_file, &symlink_path).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&test_file, &symlink_path).unwrap();

        assert_matches!(remove_symlinks(out_dir.path()), Ok(()));
        assert_matches!(fs::symlink_metadata(symlink_path), Err(e) if e.kind() == std::io::ErrorKind::NotFound);
        assert_matches!(fs::symlink_metadata(test_file), Ok(meta) if meta.is_file());
    }
}
