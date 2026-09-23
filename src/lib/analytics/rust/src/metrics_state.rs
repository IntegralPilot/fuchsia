// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::AnalyticsError;
use std::fs::{File, create_dir_all, read_to_string, remove_file};
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::env_info::is_googler;

const OPT_IN_STATUS_FILENAME: &str = "analytics-status";
const ANALYTICS_STATUS_INTERNAL_FILENAME: &str = "analytics-status-internal";

pub const UNKNOWN_APP_NAME: &str = "unknown_app";
pub const UNKNOWN_VERSION: &str = "unknown build version";
pub const UNKNOWN_PROPERTY_ID: &str = "unknown ga property id";
pub const UNKNOWN_GA4_PRODUCT_CODE: &str = "unknown ga4 property";
pub const UNKNOWN_GA4_KEY: &str = "unknown ga4 key";

/// Maintains and memo-izes the operational state of the analytics service for the app.
/// TODO(https://fxbug.dev/42077438) Once we turn down UA analytics, ~July 2023, remove ga_product_code.
#[derive(Clone, Debug, PartialEq)]
pub struct MetricsState {
    pub(crate) app_name: String,
    pub(crate) build_version: String,
    pub(crate) sdk_version: String,
    pub(crate) ga_product_code: String,
    pub(crate) ga4_product_code: String,
    pub(crate) ga4_key: String,
    pub(crate) status: MetricsStatus,
    pub(crate) uuid: Option<Uuid>,
    metrics_dir: PathBuf,
    pub(crate) invoker: Option<String>,
    pub(crate) is_new_internal_user: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MetricsStatus {
    Disabled,                     // the environment is set to turn off analytics
    NewUser,            // user has never seen the full analytics notice for the Fuchsia tools
    NewToTool,          // user has never seen the brief analytics notice for this tool
    OptedIn,            // user is allowing collection of basic analytics data
    OptedOut,           // user has opted out of collection of analytics data
    OptedInEnhanced,    // user is a googler opted in to enhanced analytics
    GooglerNeedsNotice, // user is a googler yet to set an analytics state and will be shown the enhanced analytics notice
    GooglerOptedInAndNeedsNotice, // user is a googler yet to set an analytics state and will be shown the enhanced analytics notice and we will collect basic analytics
}

impl MetricsStatus {
    pub(crate) fn is_opted_in(&self) -> bool {
        match self {
            MetricsStatus::OptedIn
            | MetricsStatus::OptedInEnhanced
            | MetricsStatus::NewToTool
            | MetricsStatus::GooglerOptedInAndNeedsNotice => true,
            _ => false,
        }
    }
}

impl MetricsState {
    pub(crate) fn from_config(
        metrics_dir: &PathBuf,
        app_name: String,
        build_version: String,
        sdk_version: String,
        ga_product_code: String,
        ga4_product_code: String,
        ga4_key: String,
        disabled: bool,
        invoker: Option<String>,
    ) -> Self {
        MetricsState::new(
            metrics_dir,
            app_name,
            build_version,
            sdk_version,
            ga_product_code,
            ga4_product_code,
            ga4_key,
            disabled,
            invoker,
        )
    }

    pub(crate) fn new(
        metrics_dir: &PathBuf,
        app_name: String,
        build_version: String,
        sdk_version: String,
        ga_product_code: String,
        ga4_product_code: String,
        ga4_key: String,
        disabled: bool,
        invoker: Option<String>,
    ) -> MetricsState {
        let mut metrics = MetricsState::default();
        if disabled {
            metrics.status = MetricsStatus::Disabled;
            return metrics;
        }
        metrics.app_name = app_name;
        metrics.build_version = build_version;
        metrics.sdk_version = sdk_version;
        metrics.metrics_dir = PathBuf::from(metrics_dir);
        metrics.ga_product_code = ga_product_code;
        metrics.ga4_product_code = ga4_product_code;
        metrics.ga4_key = ga4_key;
        metrics.invoker = invoker;
        metrics.init_status();
        metrics
    }

    pub(crate) fn set_opt_in_status(&mut self, opt_in: bool) -> Result<(), AnalyticsError> {
        if self.status == MetricsStatus::Disabled {
            return Ok(());
        }

        match opt_in {
            true => self.status = MetricsStatus::OptedIn,
            false => self.status = MetricsStatus::OptedOut,
        }
        write_opt_in_status(&self.metrics_dir, opt_in)?;
        match self.status {
            MetricsStatus::OptedOut => {
                self.uuid = None;
                delete_uuid_file(&self.metrics_dir)?;
                delete_app_file(&self.metrics_dir, &self.app_name)?;
            }
            MetricsStatus::OptedIn => {
                let uuid = Uuid::new_v4();
                self.uuid = Some(uuid);

                write_uuid_file(&self.metrics_dir, &uuid.to_string())?;
                write_app_status(&self.metrics_dir, &self.app_name, true)?;
            }
            _ => (),
        };
        Ok(())
    }

    pub(crate) fn set_new_opt_in_status(
        &mut self,
        status: MetricsStatus,
    ) -> Result<(), AnalyticsError> {
        if self.status == MetricsStatus::Disabled {
            return Ok(());
        }

        self.status = status;
        if is_googler() {
            write_analytics_status_internal(&self.metrics_dir, &self.status)?;
        } else {
            // TODO unify optin state to one file
            if self.status == MetricsStatus::OptedInEnhanced {
                // non googlers cannot enable enhanced analytics
                self.status = MetricsStatus::OptedIn;
            }
            let opted_in = self.status == MetricsStatus::OptedIn;
            write_opt_in_status(&self.metrics_dir, opted_in)?;
        }
        match self.status {
            MetricsStatus::OptedOut => {
                self.uuid = None;
                delete_uuid_file(&self.metrics_dir)?;
                delete_app_file(&self.metrics_dir, &self.app_name)?;
            }
            MetricsStatus::OptedIn | MetricsStatus::OptedInEnhanced => {
                let uuid = Uuid::new_v4();
                self.uuid = Some(uuid);

                write_uuid_file(&self.metrics_dir, &uuid.to_string())?;
                write_app_status(&self.metrics_dir, &self.app_name, true)?;
            }
            _ => (),
        };
        Ok(())
    }

    // disable analytics for this invocation only
    // this does not affect the global analytics state
    pub fn opt_out_for_this_invocation(&mut self) -> Result<(), AnalyticsError> {
        if self.status == MetricsStatus::Disabled {
            return Ok(());
        }
        self.status = MetricsStatus::OptedOut;
        Ok(())
    }

    fn init_status(&mut self) {
        if !is_googler() {
            self.init_status_for_non_googler()
        } else {
            self.init_status_for_googler()
        }
    }

    /// In the current design, non googlers' analytic status
    /// will be maintained in the old file, analytics-state.
    /// TODO redesign this to be unified with googler status
    /// as well as backward compatible.
    fn init_status_for_non_googler(&mut self) {
        match read_opt_in_status(Path::new(&self.metrics_dir)) {
            Ok(true) => self.status = MetricsStatus::OptedIn,
            Ok(false) => {
                self.status = MetricsStatus::OptedOut;
                return;
            }
            Err(_) => {
                self.status = MetricsStatus::NewUser;
            }
        }
        // write the state to the analytics-status
        if let Err(e) =
            write_opt_in_status(&self.metrics_dir, self.status != MetricsStatus::OptedOut)
        {
            eprintln!("Could not write opt in status to analytics-status {:}", e);
        }

        if self.status == MetricsStatus::OptedOut {
            return;
        }

        self.init_uuid();

        if self.status == MetricsStatus::NewUser {
            // record usage of the app on disk, but, stay 'NewUser' to prevent collection on first usage.
            if let Err(e) = write_app_status(&self.metrics_dir, &self.app_name, true) {
                eprintln!("Could not write app file,  {}, {:}", self.app_name, e);
            }
            return;
        }

        if let Err(_e) = read_app_status(&self.metrics_dir, &self.app_name) {
            self.status = MetricsStatus::NewToTool;
            if let Err(e) = write_app_status(&self.metrics_dir, &self.app_name, true) {
                eprintln!("Could not write app file,  {}, {:}", self.app_name, e);
            }
        }
    }

    fn init_status_for_googler(&mut self) {
        if let Ok(status) = read_analytics_status_internal(&self.metrics_dir) {
            log::trace!("Found status in analytics-status-internal");
            match status {
                MetricsStatus::OptedIn
                | MetricsStatus::OptedInEnhanced
                | MetricsStatus::OptedOut => {
                    self.status = status;
                }
                _ => {
                    log::trace!("Empty value found in analytics-status-internal");
                    self.status = MetricsStatus::GooglerNeedsNotice;
                }
            }
            if self.status != MetricsStatus::OptedOut {
                self.init_uuid();
            }
            return;
        }

        log::trace!("No value found in analytics-status-internal.");
        // if analytics-internal-status was not found, look for older, analytics-status file
        match read_opt_in_status(Path::new(&self.metrics_dir)) {
            Ok(true) => {
                self.status = MetricsStatus::GooglerOptedInAndNeedsNotice;
                self.init_uuid();
            }
            Ok(false) => {
                self.status = MetricsStatus::OptedOut;
            }
            Err(_) => {
                self.status = MetricsStatus::GooglerNeedsNotice;
                self.set_internal_new_user();
            }
        }
        // write the state to the new analytics-internal-status if it is already settled.
        // If the googler still has not opted in, do not write a status file.
        // TODO unify this with non-googler tracking by keeping the state in the old analytics-status file.
        // and by explicitly tracking whether the googler has seen the notice and responded to it as two
        // separate variables.
        if self.status != MetricsStatus::GooglerNeedsNotice
            && self.status != MetricsStatus::GooglerOptedInAndNeedsNotice
        {
            if let Err(e) = write_analytics_status_internal(&self.metrics_dir, &self.status) {
                eprintln!("Could not write opt in status to analytics-status-internal {:}", e);
            }
        }
        // Do not set app_status. It is not a valid state for a googler.
    }

    fn init_uuid(&mut self) {
        match read_uuid_file(&self.metrics_dir) {
            Ok(uuid) => self.uuid = Some(uuid),
            Err(_) => {
                let uuid = Uuid::new_v4();
                self.uuid = Some(uuid);

                if let Err(e) = write_uuid_file(&self.metrics_dir, &uuid.to_string()) {
                    eprintln!("Could not write uuid file {:}", e);
                }
            }
        }
    }

    fn set_internal_new_user(&mut self) {
        self.init_uuid();
        self.is_new_internal_user = true;
    }
}

impl Default for MetricsState {
    fn default() -> Self {
        MetricsState {
            app_name: String::from(UNKNOWN_APP_NAME),
            build_version: String::from(UNKNOWN_VERSION),
            sdk_version: String::from(UNKNOWN_VERSION),
            ga_product_code: UNKNOWN_PROPERTY_ID.to_string(),
            ga4_product_code: UNKNOWN_GA4_PRODUCT_CODE.to_string(),
            ga4_key: UNKNOWN_GA4_KEY.to_string(),
            status: MetricsStatus::NewUser,
            uuid: None,
            metrics_dir: PathBuf::from("/tmp"),
            invoker: None,
            is_new_internal_user: false,
        }
    }
}

fn read_opt_in_status(metrics_dir: &Path) -> Result<bool, AnalyticsError> {
    let status_file = metrics_dir.join(OPT_IN_STATUS_FILENAME);
    read_bool_from(&status_file)
}

pub(crate) fn write_opt_in_status(
    metrics_dir: &PathBuf,
    status: bool,
) -> Result<(), AnalyticsError> {
    create_dir_all(&metrics_dir)?;

    let status_file = metrics_dir.join(OPT_IN_STATUS_FILENAME);
    write_bool_to(&status_file, status)
}

fn read_app_status(metrics_dir: &PathBuf, app: &str) -> Result<bool, AnalyticsError> {
    let status_file = metrics_dir.join(app);
    read_bool_from(&status_file)
}

pub fn write_app_status(
    metrics_dir: &PathBuf,
    app: &str,
    status: bool,
) -> Result<(), AnalyticsError> {
    create_dir_all(&metrics_dir)?;

    let status_file = metrics_dir.join(app);
    write_bool_to(&status_file, status)
}

fn read_analytics_status_internal(metrics_dir: &Path) -> Result<MetricsStatus, AnalyticsError> {
    let status_file = metrics_dir.join(ANALYTICS_STATUS_INTERNAL_FILENAME);
    read_metric_status_from(&status_file)
}

fn read_metric_status_from(path: &PathBuf) -> Result<MetricsStatus, AnalyticsError> {
    let result = read_to_string(path)?;
    let parse: &str = &result.trim_end();
    let status = match parse {
        "0" => MetricsStatus::OptedOut,
        "1" => MetricsStatus::OptedIn,
        "2" => MetricsStatus::OptedInEnhanced,
        _ => MetricsStatus::NewUser,
    };
    Ok(status)
}

pub(crate) fn write_analytics_status_internal(
    metrics_dir: &PathBuf,
    status: &MetricsStatus,
) -> Result<(), AnalyticsError> {
    create_dir_all(&metrics_dir)?;

    let status_file = metrics_dir.join(ANALYTICS_STATUS_INTERNAL_FILENAME);
    write_metric_status_to(&status_file, status)
}

fn write_metric_status_to(path: &PathBuf, status: &MetricsStatus) -> Result<(), AnalyticsError> {
    let tmp_file = File::create(path)?;
    let value = match status {
        MetricsStatus::OptedOut => "0",
        MetricsStatus::OptedIn => "1",
        MetricsStatus::OptedInEnhanced => "2",
        _ => "",
    };
    writeln!(&tmp_file, "{}", value)?;
    Ok(())
}

fn read_bool_from(path: &PathBuf) -> Result<bool, AnalyticsError> {
    let result = read_to_string(path)?;
    let parse = &result.trim_end().parse::<u8>()?;
    Ok(*parse != 0)
}

fn write_bool_to(status_file_path: &PathBuf, state: bool) -> Result<(), AnalyticsError> {
    let tmp_file = File::create(status_file_path)?;
    writeln!(&tmp_file, "{}", state as u8)?;
    Ok(())
}

fn read_uuid_file(metrics_dir: &PathBuf) -> Result<Uuid, AnalyticsError> {
    let file = metrics_dir.join("uuid");
    let path = file.as_path();
    let result = read_to_string(path)?;
    match Uuid::parse_str(result.trim_end()) {
        Ok(uuid) => Ok(uuid),
        Err(e) => Err(AnalyticsError::Uuid(e)),
    }
}

fn delete_uuid_file(metrics_dir: &PathBuf) -> Result<(), AnalyticsError> {
    delete_app_file(metrics_dir, "uuid")
}

fn delete_app_file(metrics_dir: &PathBuf, app: &str) -> Result<(), AnalyticsError> {
    let file = metrics_dir.join(app);
    let path = file.as_path();
    if file.exists() { Ok(remove_file(path)?) } else { Ok(()) }
}

fn write_uuid_file(dir: &PathBuf, uuid: &str) -> Result<(), AnalyticsError> {
    create_dir_all(&dir)?;
    let file_obj = &dir.join(&"uuid");
    let uuid_file_path = file_obj.as_path();
    let uuid_file = File::create(uuid_file_path)?;
    writeln!(&uuid_file, "{}", uuid)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::metadata;
    use tempfile::tempdir;

    const APP_NAME: &str = "ffx";
    const BUILD_VERSION: &str = "12/09/20 00:00:00";
    const SDK_VERSION: &str = "99.99.99.99.1";

    #[test]
    fn new_metrics() {
        let _m = MetricsState {
            app_name: String::from(APP_NAME),
            build_version: String::from(BUILD_VERSION),
            sdk_version: String::from(SDK_VERSION),
            ga_product_code: UNKNOWN_PROPERTY_ID.to_string(),
            ga4_product_code: UNKNOWN_GA4_PRODUCT_CODE.to_string(),
            ga4_key: UNKNOWN_GA4_KEY.to_string(),
            status: MetricsStatus::NewUser,
            uuid: Some(Uuid::new_v4()),
            metrics_dir: PathBuf::from("/tmp"),
            invoker: None,
            is_new_internal_user: false,
        };
    }

    #[test]
    fn new_user_of_any_tool() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let dir = create_tmp_metrics_dir()?;
        let m = MetricsState::new(
            &dir,
            String::from(APP_NAME),
            String::from(BUILD_VERSION),
            String::from(SDK_VERSION),
            UNKNOWN_PROPERTY_ID.to_string(),
            UNKNOWN_GA4_PRODUCT_CODE.to_string(),
            UNKNOWN_GA4_KEY.to_string(),
            false,
            None,
        );
        if !is_googler() {
            assert_eq!(m.status, MetricsStatus::NewUser);
        } else {
            assert_eq!(m.status, MetricsStatus::GooglerNeedsNotice);
        }
        let result = read_uuid_file(&dir);
        match result {
            Ok(uuid) => {
                assert_eq!(m.uuid, Some(uuid));
            }
            Err(_) => panic!("Could not read uuid"),
        }

        drop(dir);
        Ok(())
    }

    #[test]
    fn existing_user_first_use_of_this_tool() -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    {
        let dir = create_tmp_metrics_dir()?;
        write_opt_in_status(&dir, true)?;

        let uuid = Uuid::default();
        write_uuid_file(&dir, &uuid.to_string())?;

        let m = MetricsState::new(
            &dir,
            String::from(APP_NAME),
            String::from(BUILD_VERSION),
            String::from(SDK_VERSION),
            UNKNOWN_PROPERTY_ID.to_string(),
            UNKNOWN_GA4_PRODUCT_CODE.to_string(),
            UNKNOWN_GA4_KEY.to_string(),
            false,
            None,
        );

        assert_ne!(Some(&m), None);
        if !is_googler() {
            assert_eq!(m.status, MetricsStatus::NewToTool);
        } else {
            assert_eq!(m.status, MetricsStatus::GooglerOptedInAndNeedsNotice);
        }
        assert_eq!(m.uuid, Some(uuid));

        let app_status_file = &dir.join(&APP_NAME);
        if !is_googler() {
            assert!(metadata(app_status_file).is_ok(), "App status file should exist.");
        } else {
            assert!(
                metadata(app_status_file).is_err(),
                "App status file should not exist when is_googler."
            );
        }

        drop(dir);
        Ok(())
    }

    #[test]
    fn existing_user_of_this_tool_opted_in() -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    {
        let dir = create_tmp_metrics_dir()?;
        write_opt_in_status(&dir, true)?;
        write_app_status(&dir, APP_NAME, true)?;
        let uuid = Uuid::default();
        write_uuid_file(&dir, &uuid.to_string())?;

        let m = MetricsState::new(
            &dir,
            String::from(APP_NAME),
            String::from(BUILD_VERSION),
            String::from(SDK_VERSION),
            UNKNOWN_PROPERTY_ID.to_string(),
            UNKNOWN_GA4_PRODUCT_CODE.to_string(),
            UNKNOWN_GA4_KEY.to_string(),
            false,
            None,
        );

        assert_ne!(Some(&m), None);
        if !is_googler() {
            assert_eq!(m.status, MetricsStatus::OptedIn);
        } else {
            assert_eq!(m.status, MetricsStatus::GooglerOptedInAndNeedsNotice);
        }
        assert_eq!(m.uuid, Some(uuid));

        drop(dir);
        Ok(())
    }

    #[test]
    fn existing_user_of_this_tool_opted_out() -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    {
        let dir = create_tmp_metrics_dir()?;
        write_opt_in_status(&dir, false)?;
        write_app_status(&dir, &APP_NAME, true)?;

        let m = MetricsState::new(
            &dir,
            String::from(APP_NAME),
            String::from(BUILD_VERSION),
            String::from(SDK_VERSION),
            UNKNOWN_PROPERTY_ID.to_string(),
            UNKNOWN_GA4_PRODUCT_CODE.to_string(),
            UNKNOWN_GA4_KEY.to_string(),
            false,
            None,
        );

        assert_ne!(Some(&m), None);
        assert_eq!(m.status, MetricsStatus::OptedOut);
        assert_eq!(m.uuid, None);

        drop(dir);
        Ok(())
    }

    #[test]
    fn with_disable_env_var_set() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let dir = create_tmp_metrics_dir()?;
        let m = MetricsState::new(
            &dir,
            String::from(APP_NAME),
            String::from(BUILD_VERSION),
            String::from(SDK_VERSION),
            UNKNOWN_PROPERTY_ID.to_string(),
            UNKNOWN_GA4_PRODUCT_CODE.to_string(),
            UNKNOWN_GA4_KEY.to_string(),
            true,
            None,
        );

        assert_eq!(m.status, MetricsStatus::Disabled);
        assert_eq!(m.uuid, None);
        Ok(())
    }

    #[test]
    fn existing_user_of_this_tool_opted_in_then_out_then_in()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let dir = create_tmp_metrics_dir()?;
        write_opt_in_status(&dir, true)?;
        write_app_status(&dir, &APP_NAME, true)?;
        let uuid = Uuid::default();
        write_uuid_file(&dir, &uuid.to_string())?;
        let mut m = MetricsState::new(
            &dir,
            String::from(APP_NAME),
            String::from(BUILD_VERSION),
            String::from(SDK_VERSION),
            UNKNOWN_PROPERTY_ID.to_string(),
            UNKNOWN_GA4_PRODUCT_CODE.to_string(),
            UNKNOWN_GA4_KEY.to_string(),
            false,
            None,
        );

        assert_ne!(Some(&m), None);
        if !is_googler() {
            assert_eq!(m.status, MetricsStatus::OptedIn);
        } else {
            assert_eq!(m.status, MetricsStatus::GooglerOptedInAndNeedsNotice);
        }
        assert_eq!(m.uuid, Some(uuid));

        m.set_opt_in_status(false)?;

        assert_eq!(m.status, MetricsStatus::OptedOut);
        assert_eq!(m.uuid, None);
        let app_status_file = &dir.join(&APP_NAME);
        assert!(metadata(app_status_file).is_err(), "App status file should not exist.");

        m.set_opt_in_status(true)?;

        assert_eq!(m.status, MetricsStatus::OptedIn);
        assert_eq!(m.uuid, Some(read_uuid_file(&dir).unwrap()));
        assert_eq!(true, read_app_status(&dir, &APP_NAME)?);

        drop(dir);
        Ok(())
    }

    pub fn create_tmp_metrics_dir() -> Result<PathBuf, AnalyticsError> {
        let tmp_dir = tempdir().map_err(AnalyticsError::Io)?;
        let dir_obj = tmp_dir.path().join("fuchsia_metrics");
        let dir = dir_obj.as_path();
        Ok(dir.to_owned())
    }
}
