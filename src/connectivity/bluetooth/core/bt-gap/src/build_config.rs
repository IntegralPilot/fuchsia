// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{format_err, Error};
use fidl_fuchsia_bluetooth_sys::{self as sys, BrEdrSecurityMode, LeSecurityMode};
use std::cmp::PartialEq;

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Config {
    pub le: LeConfig,
    pub bredr: BrEdrConfig,
}

impl TryFrom<bt_gap_config::Config> for Config {
    type Error = Error;
    fn try_from(value: bt_gap_config::Config) -> Result<Self, Self::Error> {
        Ok(Self {
            le: LeConfig {
                privacy_enabled: value.le_privacy,
                background_scan_enabled: value.le_background_scanning,
                security_mode: le_security_mode_from_str(value.le_security_mode)?,
            },
            bredr: BrEdrConfig {
                connectable: value.bredr_connectable,
                security_mode: bredr_security_mode_from_str(value.bredr_security_mode)?,
            },
        })
    }
}

fn le_security_mode_from_str(security_mode: String) -> Result<LeSecurityMode, Error> {
    match security_mode.as_str() {
        "Mode1" => Ok(LeSecurityMode::Mode1),
        "SecureConnectionsOnly" => Ok(LeSecurityMode::SecureConnectionsOnly),
        x => Err(format_err!("Unrecognized le_security_mode: {x:?}")),
    }
}

fn bredr_security_mode_from_str(security_mode: String) -> Result<BrEdrSecurityMode, Error> {
    match security_mode.as_str() {
        "Mode4" => Ok(BrEdrSecurityMode::Mode4),
        "SecureConnectionsOnly" => Ok(BrEdrSecurityMode::SecureConnectionsOnly),
        x => Err(format_err!("Unrecognized bredr_security_mode: {x:?}")),
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct LeConfig {
    pub privacy_enabled: bool,
    pub background_scan_enabled: bool,
    pub security_mode: LeSecurityMode,
}

impl Default for LeConfig {
    fn default() -> Self {
        Self {
            privacy_enabled: true,
            background_scan_enabled: true,
            security_mode: LeSecurityMode::Mode1,
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct BrEdrConfig {
    pub connectable: bool,
    pub security_mode: BrEdrSecurityMode,
}

impl Default for BrEdrConfig {
    fn default() -> Self {
        Self { connectable: true, security_mode: BrEdrSecurityMode::Mode4 }
    }
}

impl Config {
    pub fn update_with_sys_settings(&self, new_settings: &sys::Settings) -> Self {
        let mut new_config = self.clone();
        new_config.le.privacy_enabled = new_settings.le_privacy.unwrap_or(self.le.privacy_enabled);
        new_config.le.background_scan_enabled =
            new_settings.le_background_scan.unwrap_or(self.le.background_scan_enabled);
        new_config.le.security_mode =
            new_settings.le_security_mode.unwrap_or(self.le.security_mode);
        new_config.bredr.connectable =
            new_settings.bredr_connectable_mode.unwrap_or(self.bredr.connectable);
        new_config.bredr.security_mode =
            new_settings.bredr_security_mode.unwrap_or(self.bredr.security_mode);
        new_config
    }
}

impl Into<sys::Settings> for Config {
    fn into(self) -> sys::Settings {
        sys::Settings {
            le_privacy: Some(self.le.privacy_enabled),
            le_background_scan: Some(self.le.background_scan_enabled),
            bredr_connectable_mode: Some(self.bredr.connectable),
            le_security_mode: Some(self.le.security_mode),
            bredr_security_mode: Some(self.bredr.security_mode),
            ..Default::default()
        }
    }
}

pub fn load_default() -> Result<Config, Error> {
    bt_gap_config::Config::take_from_startup_handle().try_into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host_device::HostDevice;
    use assert_matches::assert_matches;
    use fidl_fuchsia_bluetooth_host::{HostMarker, HostRequest};
    use fuchsia_bluetooth::types::{Address, HostId};
    use futures::stream::TryStreamExt;
    use futures::{future, join};
    use std::collections::HashSet;

    static BASIC_CONFIG: Config = Config {
        le: LeConfig {
            privacy_enabled: true,
            background_scan_enabled: true,
            security_mode: LeSecurityMode::Mode1,
        },
        bredr: BrEdrConfig { connectable: true, security_mode: BrEdrSecurityMode::Mode4 },
    };

    #[fuchsia::test]
    async fn apply_config() {
        let (host_proxy, host_server) = fidl::endpoints::create_proxy_and_stream::<HostMarker>();
        let host_device = HostDevice::mock(
            HostId(42),
            Address::Public([1, 2, 3, 4, 5, 6]),
            "/dev/host".to_string(),
            host_proxy,
        );
        let test_config = Config {
            le: LeConfig {
                privacy_enabled: true,
                background_scan_enabled: false,
                security_mode: LeSecurityMode::Mode1,
            },
            bredr: BrEdrConfig { connectable: false, security_mode: BrEdrSecurityMode::Mode4 },
        };
        let run_host = async {
            let mut expected_reqs = HashSet::<String>::new();
            let _ = expected_reqs.insert("enable_privacy".into());
            let _ = expected_reqs.insert("enable_background_scan".into());
            let _ = expected_reqs.insert("set_connectable".into());
            let _ = expected_reqs.insert("set_le_security_mode".into());
            let _ = expected_reqs.insert("set_bredr_security_mode".into());
            host_server
                .try_for_each(|req| {
                    match req {
                        HostRequest::EnablePrivacy { enabled, .. } => {
                            assert!(expected_reqs.remove("enable_privacy"));
                            assert_eq!(test_config.le.privacy_enabled, enabled);
                        }
                        HostRequest::EnableBackgroundScan { enabled, .. } => {
                            assert!(expected_reqs.remove("enable_background_scan"));
                            assert_eq!(test_config.le.background_scan_enabled, enabled);
                        }
                        HostRequest::SetConnectable { enabled, responder } => {
                            assert!(expected_reqs.remove("set_connectable"));
                            assert_eq!(test_config.bredr.connectable, enabled);
                            assert_matches!(responder.send(Ok(())), Ok(()));
                        }
                        HostRequest::SetLeSecurityMode { le_security_mode, .. } => {
                            assert!(expected_reqs.remove("set_le_security_mode"));
                            assert_eq!(test_config.le.security_mode, le_security_mode);
                        }
                        HostRequest::SetBrEdrSecurityMode { bredr_security_mode, .. } => {
                            assert!(expected_reqs.remove("set_bredr_security_mode"));
                            assert_eq!(test_config.bredr.security_mode, bredr_security_mode);
                        }
                        HostRequest::SetBondingDelegate { .. } => {}
                        _ => panic!("unexpected HostRequest!"),
                    };
                    future::ok(())
                })
                .await
                .unwrap();
            assert_eq!(expected_reqs.into_iter().collect::<Vec<String>>(), Vec::<String>::new());
        };
        let apply_config = async {
            host_device.apply_config(test_config.clone()).await.unwrap();
            // Drop `host_device` so the host server request stream terminates
            drop(host_device)
        };
        join!(run_host, apply_config);
    }

    #[test]
    fn update_with_sys_settings() {
        let partial_settings = sys::Settings {
            le_privacy: Some(false),
            bredr_connectable_mode: Some(false),
            ..Default::default()
        };
        let expected_config = Config {
            le: LeConfig { privacy_enabled: false, ..BASIC_CONFIG.le },
            bredr: BrEdrConfig { connectable: false, ..BASIC_CONFIG.bredr },
        };
        assert_eq!(
            expected_config,
            BASIC_CONFIG.clone().update_with_sys_settings(&partial_settings)
        );
    }
}
