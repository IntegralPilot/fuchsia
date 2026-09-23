// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::config;
use crate::driver::Driver;
use crate::finder::{Answer, Finder};
use crate::net::IpAddr;

use anyhow::format_err;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use home::home_dir;

const TESTBED_NAME: &'static str = "antlion-runner";

/// Driver for running antlion locally on an emulated or hardware testbed with
/// optional mDNS discovery when a DHCP server is not available. This is useful
/// for testing changes locally in a development environment.
pub(crate) struct LocalDriver {
    target: LocalTarget,
    access_point: Option<LocalAccessPoint>,
    output_dir: PathBuf,
    ssh_binary: PathBuf,
    ffx_binary: PathBuf,
    ffx_subtools_search_path: Option<PathBuf>,
}

impl LocalDriver {
    pub fn new<F>(
        finder: F,
        device: Option<String>,
        ssh_binary: PathBuf,
        ssh_key: Option<PathBuf>,
        ffx_binary: PathBuf,
        ffx_subtools_search_path: Option<PathBuf>,
        out_dir: Option<PathBuf>,
        ap_ip: Option<String>,
        ap_ssh_port: Option<u16>,
        ap_ssh_key: Option<PathBuf>,
    ) -> Result<Self>
    where
        F: Finder,
    {
        let output_dir = match out_dir {
            Some(p) => Ok(p),
            None => std::env::current_dir().context("Failed to get current working directory"),
        }?;

        let target = LocalTarget::new(finder, device, ssh_key)?;

        // If an access point IP has been provided, try to derive other AP-related parameters
        let access_point = if let Some(ip_str) = ap_ip {
            let ssh_port = ap_ssh_port.unwrap_or_else(|| {
                let default_ssh_port = 22;
                println!("AP IP provided without AP SSH port, assuming {default_ssh_port}");
                default_ssh_port
            });
            let ssh_key = match ap_ssh_key {
                Some(path) => Ok(path),
                None => match find_ap_ssh_key() {
                    Ok(path) => {
                        println!("Using AP SSH key found at {}", path.display());
                        Ok(path)
                    }
                    Err(e) => Err(e),
                },
            }?;
            Some(LocalAccessPoint {
                ip: ip_str.parse::<IpAddr>().expect("Failed to parse AP IP address"),
                ssh_port: Some(ssh_port),
                ssh_key,
            })
        } else {
            None
        };

        Ok(Self {
            target,
            access_point,
            output_dir,
            ssh_binary,
            ffx_binary,
            ffx_subtools_search_path,
        })
    }
}

fn find_ap_ssh_key() -> Result<PathBuf> {
    // Look for the SSH key at some known paths
    let home_dir = std::env::var("HOME").map_err(|_| {
        format_err!(
            "AP IP was provided, but AP SSH key not provided and could not be automatically found"
        )
    })?;
    let home_dir = Path::new(&home_dir);
    let ssh_key_search_paths =
        [home_dir.join(".ssh/onhub_testing_rsa"), home_dir.join(".ssh/testing_rsa")];
    for path in ssh_key_search_paths.clone() {
        if path.exists() {
            return Ok(path);
        }
    }
    let ssh_key_search_paths =
        ssh_key_search_paths.map(|p| p.to_string_lossy().into_owned()).join(", ");
    return Err(format_err!(
        "AP IP is provided, but AP SSH key was not provided, and not found in default locations: [{}]",
        ssh_key_search_paths
    ));
}

impl Driver for LocalDriver {
    fn output_path(&self) -> &Path {
        self.output_dir.as_path()
    }
    fn config(&self) -> config::Config {
        let mut access_points = vec![];
        if let Some(ref ap) = self.access_point {
            access_points.push(config::AccessPoint {
                wan_interface: "eth0".to_string(),
                nodename: None,
                ssh_config: config::SshConfig {
                    ssh_binary_path: self.ssh_binary.clone(),
                    host: ap.ip.clone(),
                    port: ap.ssh_port,
                    user: "root".to_string(),
                    identity_file: ap.ssh_key.clone(),
                },
                pdu_device: None,
                attenuators: None,
                allow_regdb_bypass: Some(false),
            });
        }

        config::Config {
            testbeds: vec![config::Testbed {
                name: TESTBED_NAME.to_string(),
                controllers: config::Controllers {
                    fuchsia_devices: vec![config::Fuchsia {
                        name: self.target.name.clone(),
                        ip: self.target.ip.clone(),
                        device_ip_port: match self.target.ssh_port {
                            None => format!("{}", self.target.ip),
                            Some(port) => format!("{}:{}", self.target.ip, port),
                        },
                        take_bug_report_on_fail: false,
                        ssh_port: self.target.ssh_port.clone(),
                        ssh_binary_path: self.ssh_binary.clone(),
                        // TODO(http://b/244747218): Remove when ssh_config is refactored away
                        ssh_config: None,
                        ffx_binary_path: self.ffx_binary.clone(),
                        ffx_subtools_search_path: self.ffx_subtools_search_path.clone(),
                        ssh_priv_key: self.target.ssh_key.clone(),
                        pdu_device: None,
                        hard_reboot_on_fail: false,
                        honeydew_config: config::HoneydewConfig {
                            transports: config::HoneydewTransports {
                                ffx: config::HoneydewFfx {
                                    path: self.ffx_binary.clone(),
                                    subtools_search_path: self.ffx_subtools_search_path.clone(),
                                },
                            },
                            affordances: config::HoneydewAffordances {
                                bluetooth: config::HoneydewAffordanceSpec {
                                    implementation: config::HONEYDEW_IMPL_FUCHSIA_CONTROLLER
                                        .to_string(),
                                },
                            },
                        },
                    }],
                    access_points: access_points,
                    ..Default::default()
                },
                test_params: None,
            }],
            mobly_params: config::MoblyParams { log_path: self.output_dir.clone() },
        }
    }
    fn setup(&self) -> Result<()> {
        Ok(())
    }
    fn teardown(&self) -> Result<()> {
        println!(
            "\nView full antlion logs at {}",
            self.output_dir.join(TESTBED_NAME).join("latest").display()
        );
        Ok(())
    }
}

struct LocalAccessPoint {
    ip: IpAddr,
    ssh_port: Option<u16>,
    ssh_key: PathBuf,
}

/// LocalTargetInfo performs best-effort discovery of target information from
/// standard Fuchsia environmental variables.
struct LocalTarget {
    name: String,
    ip: IpAddr,
    ssh_port: Option<u16>,
    ssh_key: PathBuf,
}

impl LocalTarget {
    fn new<F: Finder>(finder: F, device: Option<String>, ssh_key: Option<PathBuf>) -> Result<Self> {
        let Answer { name, ip, ssh_port } = finder.find_device(device)?;

        // TODO: Move this validation out to Args
        let ssh_key = ssh_key
            .or_else(|| home_dir().map(|p| p.join(".ssh/fuchsia_ed25519")))
            .context("Failed to detect the private Fuchsia SSH key")?;

        ensure!(
            ssh_key.try_exists().with_context(|| format!(
                "Failed to check existence of SSH key \"{}\"",
                ssh_key.display()
            ))?,
            "Cannot find SSH key \"{}\"",
            ssh_key.display()
        );

        Ok(LocalTarget { name, ip, ssh_port, ssh_key })
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use crate::generate_config_and_run;
    use crate::runner::{ExitStatus, Runner};

    use indoc::formatdoc;
    use pretty_assertions::assert_eq;
    use tempfile::{NamedTempFile, TempDir};

    const FUCHSIA_NAME: &'static str = "fuchsia-1234-5678-9abc";
    const FUCHSIA_ADDR: &'static str = "fe80::1%eth0";
    const FUCHSIA_IP: &'static str = "fe80::1";
    const FUCHSIA_IPV4: &'static str = "127.0.0.1";
    const FUCHSIA_SSH_PORT: u16 = 5002;
    const SCOPE_ID: &'static str = "eth0";

    struct MockFinder;
    impl Finder for MockFinder {
        fn find_device(&self, _: Option<String>) -> Result<Answer> {
            Ok(Answer {
                name: FUCHSIA_NAME.to_string(),
                ip: IpAddr::V6(FUCHSIA_IP.parse().unwrap(), Some(SCOPE_ID.to_string())),
                ssh_port: None,
            })
        }
    }

    struct MockFinderWithSsh;
    impl Finder for MockFinderWithSsh {
        fn find_device(&self, _: Option<String>) -> Result<Answer> {
            Ok(Answer {
                name: FUCHSIA_NAME.to_string(),
                ip: IpAddr::V4(FUCHSIA_IPV4.parse().unwrap()),
                ssh_port: Some(FUCHSIA_SSH_PORT),
            })
        }
    }

    #[derive(Default)]
    struct MockRunner {
        config: std::cell::Cell<PathBuf>,
    }
    impl Runner for MockRunner {
        fn run(&self, config: PathBuf) -> Result<ExitStatus> {
            self.config.set(config);
            Ok(ExitStatus::Ok)
        }
    }

    #[test]
    fn local_invalid_ssh_key() {
        let ssh = NamedTempFile::new().unwrap();
        let ffx = NamedTempFile::new().unwrap();
        let out_dir = TempDir::new().unwrap();

        assert!(
            LocalDriver::new(
                MockFinder {},
                None,
                ssh.path().to_path_buf(),
                Some(PathBuf::new()),
                ffx.path().to_path_buf(),
                None,
                Some(out_dir.path().to_path_buf()),
                None,
                None,
                None,
            )
            .is_err()
        );
    }

    #[test]
    fn local() {
        let ssh = NamedTempFile::new().unwrap();
        let ssh_key = NamedTempFile::new().unwrap();
        let ffx = NamedTempFile::new().unwrap();
        let ffx_subtools = TempDir::new().unwrap();
        let out_dir = TempDir::new().unwrap();

        let runner = MockRunner::default();
        let driver = LocalDriver::new(
            MockFinder {},
            None,
            ssh.path().to_path_buf(),
            Some(ssh_key.path().to_path_buf()),
            ffx.path().to_path_buf(),
            Some(ffx_subtools.path().to_path_buf()),
            Some(out_dir.path().to_path_buf()),
            None,
            None,
            None,
        )
        .unwrap();

        generate_config_and_run(runner, driver, None).unwrap();

        let got = std::fs::read_to_string(out_dir.path().join("config.yaml")).unwrap();

        let ssh_path = ssh.path().display();
        let ssh_key_path = ssh_key.path().display();
        let ffx_path = ffx.path().display();
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
              take_bug_report_on_fail: false
              ssh_binary_path: {ssh_path}
              ffx_binary_path: {ffx_path}
              ffx_subtools_search_path: {ffx_subtools_path}
              ssh_priv_key: {ssh_key_path}
              hard_reboot_on_fail: false
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
    fn local_with_ssh_port() {
        let ssh = NamedTempFile::new().unwrap();
        let ssh_key = NamedTempFile::new().unwrap();
        let ffx = NamedTempFile::new().unwrap();
        let ffx_subtools = TempDir::new().unwrap();
        let out_dir = TempDir::new().unwrap();

        let runner = MockRunner::default();
        let driver = LocalDriver::new(
            MockFinderWithSsh {},
            None,
            ssh.path().to_path_buf(),
            Some(ssh_key.path().to_path_buf()),
            ffx.path().to_path_buf(),
            Some(ffx_subtools.path().to_path_buf()),
            Some(out_dir.path().to_path_buf()),
            None,
            None,
            None,
        )
        .unwrap();

        generate_config_and_run(runner, driver, None).unwrap();

        let got = std::fs::read_to_string(out_dir.path().join("config.yaml")).unwrap();

        let ssh_path = ssh.path().display();
        let ssh_key_path = ssh_key.path().display();
        let ffx_path = ffx.path().display();
        let ffx_subtools_path = ffx_subtools.path().display();
        let out_path = out_dir.path().display();
        let want = formatdoc! {r#"
        TestBeds:
        - Name: {TESTBED_NAME}
          Controllers:
            FuchsiaDevice:
            - name: {FUCHSIA_NAME}
              ip: {FUCHSIA_IPV4}
              ssh_port: {FUCHSIA_SSH_PORT}
              device_ip_port: {FUCHSIA_IPV4}:{FUCHSIA_SSH_PORT}
              take_bug_report_on_fail: false
              ssh_binary_path: {ssh_path}
              ffx_binary_path: {ffx_path}
              ffx_subtools_search_path: {ffx_subtools_path}
              ssh_priv_key: {ssh_key_path}
              hard_reboot_on_fail: false
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
    fn local_with_test_params() {
        let ssh = NamedTempFile::new().unwrap();
        let ssh_key = NamedTempFile::new().unwrap();
        let ffx = NamedTempFile::new().unwrap();
        let ffx_subtools = TempDir::new().unwrap();
        let out_dir = TempDir::new().unwrap();

        let runner = MockRunner::default();
        let driver = LocalDriver::new(
            MockFinder {},
            None,
            ssh.path().to_path_buf(),
            Some(ssh_key.path().to_path_buf()),
            ffx.path().to_path_buf(),
            Some(ffx_subtools.path().to_path_buf()),
            Some(out_dir.path().to_path_buf()),
            None,
            None,
            None,
        )
        .unwrap();

        let params_yaml = "
        my_test_params:
            foo: bar
        ";
        let params = serde_yaml::from_str(params_yaml).unwrap();

        generate_config_and_run(runner, driver, Some(params)).unwrap();

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
              take_bug_report_on_fail: false
              ssh_binary_path: {ssh_path}
              ffx_binary_path: {ffx_path}
              ffx_subtools_search_path: {ffx_subtools_path}
              ssh_priv_key: {ssh_key_path}
              hard_reboot_on_fail: false
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
              foo: bar
        MoblyParams:
          LogPath: {out_path}
        "#};

        assert_eq!(got, want);
    }

    #[test]
    fn local_with_ap() {
        let ssh = NamedTempFile::new().unwrap();
        let ssh_key = NamedTempFile::new().unwrap();
        let ffx = NamedTempFile::new().unwrap();
        let ffx_subtools = TempDir::new().unwrap();
        let out_dir = TempDir::new().unwrap();
        let ap_ssh_key = NamedTempFile::new().unwrap();
        let ap_ssh_port: u16 = 1245;
        let ap_ip = "192.168.1.1".to_string();

        let runner = MockRunner::default();
        let driver = LocalDriver::new(
            MockFinder {},
            None,
            ssh.path().to_path_buf(),
            Some(ssh_key.path().to_path_buf()),
            ffx.path().to_path_buf(),
            Some(ffx_subtools.path().to_path_buf()),
            Some(out_dir.path().to_path_buf()),
            Some(ap_ip.clone()),
            Some(ap_ssh_port),
            Some(ap_ssh_key.path().to_path_buf()),
        )
        .unwrap();

        generate_config_and_run(runner, driver, None).unwrap();

        let got = std::fs::read_to_string(out_dir.path().join("config.yaml")).unwrap();

        let ssh_path = ssh.path().display();
        let ssh_key_path = ssh_key.path().display();
        let ap_ssh_key_path = ap_ssh_key.path().display();
        let ffx_path = ffx.path().display();
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
              take_bug_report_on_fail: false
              ssh_binary_path: {ssh_path}
              ffx_binary_path: {ffx_path}
              ffx_subtools_search_path: {ffx_subtools_path}
              ssh_priv_key: {ssh_key_path}
              hard_reboot_on_fail: false
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
              ssh_config:
                ssh_binary_path: {ssh_path}
                host: {ap_ip}
                port: {ap_ssh_port}
                user: root
                identity_file: {ap_ssh_key_path}
              allow_regdb_bypass: false
        MoblyParams:
          LogPath: {out_path}
        "#};

        assert_eq!(got, want);
    }
}
