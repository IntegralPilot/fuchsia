// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::AnalyticsError;
#[allow(unused_imports)]
use home::home_dir;
use nix::unistd;
use regex::Regex;
use std::env::VarError;
#[allow(unused_imports)]
use std::fs::symlink_metadata;
use std::fs::{copy, create_dir_all, read_dir, remove_dir_all};
#[allow(unused_imports)]
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::str::FromStr;

pub fn get_os() -> String {
    convert_macos_to_darwin(std::env::consts::OS)
}

pub fn get_arch() -> String {
    std::env::consts::ARCH.to_string()
}

pub fn get_hostname() -> String {
    unistd::gethostname().expect("$HOSTNAME").into_string().expect("$HOSTNAME")
}

fn convert_macos_to_darwin(arch: &str) -> String {
    arch.replace("macos", "Darwin")
}

///
/// Uses $XDG_DATA_HOME or ~/.local/share
/// to create the analytics directory, Fuchsia/metrics.
/// When, creating this folder, if there is a legacy analytics folder,
/// it migrates those settings.
///
pub fn get_analytics_dir() -> Result<PathBuf, AnalyticsError> {
    let analytics_folder = new_analytics_folder();
    migrate_legacy_folder(&analytics_folder)?;
    Ok(analytics_folder)
}

pub(crate) fn migrate_legacy_folder(analytics_folder: &PathBuf) -> Result<(), AnalyticsError> {
    let old_analytics_folder = old_analytics_folder();
    if !analytics_folder.exists() && old_analytics_folder.exists() {
        log::trace!("Migrating analytics to {analytics_folder:?}");
        create_dir_all(&analytics_folder).map_err(AnalyticsError::Io)?;
        copy_old_to_new(&old_analytics_folder, &analytics_folder)?;
        remove_old_analytics_dir(&old_analytics_folder);
        symlink_old_to_new(&analytics_folder, &old_analytics_folder);
    } else if !analytics_folder.exists() {
        create_dir_all(&analytics_folder).map_err(AnalyticsError::Io)?;
    }
    Ok(())
}

fn remove_old_analytics_dir(old_analytics_folder: &PathBuf) {
    match remove_dir_all(old_analytics_folder) {
        Ok(()) => log::trace!("removed old directory"),
        Err(e) => log::error!("Error removing old directory: {:?}", e),
    }
}

#[cfg(target_os = "linux")]
fn symlink_old_to_new(analytics_folder: &PathBuf, old_analytics_folder: &PathBuf) {
    let old_parent = old_analytics_folder.parent().unwrap();
    if !old_parent.exists() {
        match create_dir_all(&old_parent) {
            Ok(()) => log::trace!("Created parent of old analytics folder"),
            Err(e) => log::error!(
                "Could not create parent of old analytics directory for symlinking: {:?},{:?},{:?}",
                analytics_folder,
                old_parent,
                e
            ),
        }
    }
    if old_parent.exists() {
        match symlink(analytics_folder, old_analytics_folder) {
            Ok(()) => log::trace!("symlinked old directory to new directory"),
            Err(e) => log::error!(
                "Error symlinking old directory: {:?},{:?},{:?}",
                analytics_folder,
                old_analytics_folder.as_path(),
                e
            ),
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn symlink_old_to_new(_analytics_folder: &PathBuf, _old_analytics_folder: &PathBuf) {}

fn old_analytics_folder() -> PathBuf {
    let mut metrics_path = get_home_dir();
    metrics_path.push(".fuchsia/metrics");
    metrics_path
}

#[allow(dead_code)]
fn new_analytics_folder() -> PathBuf {
    let mut metrics_path = xdg_data_home();
    metrics_path.push("Fuchsia/metrics/");
    metrics_path
}

/// Copies legacy analytics settings to new location
fn copy_old_to_new(
    old_analytics_folder: &PathBuf,
    new_analytics_folder: &PathBuf,
) -> Result<(), AnalyticsError> {
    if !&old_analytics_folder.exists() {
        return Err(AnalyticsError::OldFolderNotFound);
    }
    for entry in read_dir(old_analytics_folder).map_err(AnalyticsError::Io)? {
        let entry = entry.map_err(AnalyticsError::Io)?;
        log::trace!("Copying file: {:?}", entry.path());
        copy(entry.path(), new_analytics_folder.as_path().join(entry.file_name()))
            .map_err(AnalyticsError::Io)?;
    }
    Ok(())
}

#[allow(dead_code)]
fn xdg_data_home() -> PathBuf {
    #[allow(unreachable_patterns)] // TODO(https://fxbug.dev/360336471)
    match xdg_data_home_env_var() {
        Ok(path) => match PathBuf::from_str(&path) {
            Ok(pb) => pb,
            Err(_e) => {
                let mut hd = get_home_dir();
                hd.push(".local/share");
                hd
            }
        },
        Err(_e) => {
            let mut hd = get_home_dir();
            hd.push(".local/share");
            hd
        }
    }
}

#[cfg(not(test))]
fn xdg_data_home_env_var() -> Result<String, VarError> {
    std::env::var("XDG_DATA_HOME")
}

#[cfg(test)]
fn xdg_data_home_env_var() -> Result<String, VarError> {
    Ok("/tmp/testinganalytics/.local/share".into())
}

#[cfg(not(test))]
fn get_home_dir() -> PathBuf {
    match home_dir() {
        Some(dir) => dir,
        None => PathBuf::from("/tmp"),
    }
}

#[cfg(test)]
fn get_home_dir() -> PathBuf {
    PathBuf::from("/tmp/testinganalytics/")
}

const TEST_ENV_VAR: &'static str = "FUCHSIA_TEST_OUTDIR";
const ANALYTICS_DISABLED_ENV_VAR: &'static str = "FUCHSIA_ANALYTICS_DISABLED";
const HOME_VAR: &'static str = "HOME";

// To detect certain bot environmments that run as USER "builder"
#[cfg(not(test))]
const USER: &'static str = "USER";
const BUILDER: &'static str = "builder";

const BOT_ENV_VARS: &'static [&'static str] = &[
    "TF_BUILD",           // Azure
    "bamboo.buildKey",    // Bamboo
    "BUILDKITE",          // BUILDKITE
    "CIRCLECI",           // Circle
    "CIRRUS_CI",          // Cirrus
    "CODEBUILD_BUILD_ID", // Codebuild
    "UNITTEST_ON_BORG",   // Borg
    "UNITTEST_ON_FORGE",  // Forge
    "SWARMING_BOT_ID",    // Fuchsia
    "GITHUB_ACTIONS",     // GitHub Actions
    "GITLAB_CI",          // GitLab
    "HEROKU_TEST_RUN_ID", // Heroku
    "BUILD_ID",           // Hudson & Jenkins
    "TEAMCITY_VERSION",   // Teamcity
    "TRAVIS",             // Travis
    "BUILD_NUMBER",       // android-ci
];

#[allow(dead_code)]
const C_GOOGLERS_DOMAIN_NAME: &str = r"c.googlers.com$";
#[allow(dead_code)]
const CORP_GOOGLERS_DOMAIN_NAME: &str = r"corp.google.com$";
#[allow(dead_code)]
const USER_REDACTED: &str = "$USER";
const HOSTNAME_REDACTED: &str = "$HOSTNAME";

pub fn is_test_env() -> bool {
    std::env::var(TEST_ENV_VAR).is_ok() || std::env::var(HOME_VAR).is_err()
}

pub fn is_fuchsia_analytics_disabled_set() -> bool {
    std::env::var(ANALYTICS_DISABLED_ENV_VAR).is_ok()
}

pub fn is_running_in_ci_bot_env() -> bool {
    BOT_ENV_VARS.iter().any(|env_var| std::env::var(env_var).is_ok())
}

pub fn is_user_a_bot() -> bool {
    get_username_from_env().is_ok_and(|v| v == BUILDER)
}

pub fn is_analytics_disabled_by_env() -> bool {
    is_test_env()
        || is_fuchsia_analytics_disabled_set()
        || is_running_in_ci_bot_env()
        || is_user_a_bot()
}

pub fn is_googler() -> bool {
    is_googler_host_domain(&get_hostname())
}

#[allow(dead_code)]
pub fn is_googler_host_domain(domain: &str) -> bool {
    static C_GOOGLERS_DOMAIN_RE: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(C_GOOGLERS_DOMAIN_NAME).unwrap());
    static CORP_GOOGLERS_DOMAIN_RE: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(CORP_GOOGLERS_DOMAIN_NAME).unwrap());
    C_GOOGLERS_DOMAIN_RE.is_match(domain) || CORP_GOOGLERS_DOMAIN_RE.is_match(domain)
}

#[allow(dead_code)]
pub(crate) fn redact_host_and_user_from(parameter: &str) -> String {
    redact_hostname_from(&redact_user_name_from(parameter))
}

#[allow(dead_code)]
pub fn redact_user_name_from(parameter: &str) -> String {
    match get_username_from_env() {
        Ok(user_value) => parameter.replace(user_value.as_str(), USER_REDACTED),
        _ => parameter.to_string(),
    }
}

#[allow(dead_code)]
pub fn redact_hostname_from(parameter: &str) -> String {
    parameter.replace(get_hostname().as_str(), HOSTNAME_REDACTED)
}

#[cfg(not(test))]
pub fn get_username_from_env() -> Result<String, VarError> {
    std::env::var(USER)
}

#[cfg(test)]
pub fn get_username_from_env() -> Result<String, VarError> {
    Ok("player1".to_string())
}

#[cfg(test)]
mod test {
    use super::*;
    #[allow(unused_imports)]
    use std::fs::{read_link, remove_dir_all, symlink_metadata, write};

    #[test]
    // TODO(https://fxbug.dev/42148443): isolate the env test from CI env
    #[ignore]
    // Rust tests are run in parallel in threads, which means that this test is
    // disruptive to other tests. There's little ROI to doing some kind of fork
    // dance here, so the test is included, but not run by default.
    pub fn test_is_test_env() {
        unsafe { std::env::set_var(TEST_ENV_VAR, "somepath") };
        assert_eq!(true, is_test_env());
        unsafe { std::env::remove_var(TEST_ENV_VAR) };
        assert_eq!(false, is_test_env());
    }

    #[test]
    #[ignore]
    // Rust tests are run in parallel in threads, which means that this test is
    // disruptive to other tests. There's little ROI to doing some kind of fork
    // dance here, so the test is included, but not run by default.
    pub fn test_is_analytics_disabled_env() {
        unsafe { std::env::set_var(ANALYTICS_DISABLED_ENV_VAR, "1") };
        assert_eq!(true, is_fuchsia_analytics_disabled_set());
        unsafe { std::env::remove_var(ANALYTICS_DISABLED_ENV_VAR) };
        assert_eq!(false, is_fuchsia_analytics_disabled_set());
    }

    #[test]
    // TODO(https://fxbug.dev/42148443): isolate the env test from CI env
    #[ignore]
    pub fn test_is_bot_env() {
        unsafe { std::env::set_var(&"BUILD_ID", "1") };
        assert_eq!(true, is_running_in_ci_bot_env());
        unsafe { std::env::remove_var(&"BUILD_ID") };
        assert_eq!(false, is_fuchsia_analytics_disabled_set());
    }

    #[test]
    pub fn test_arch_macos_conversion() {
        assert_eq!("Darwin", convert_macos_to_darwin("macos"))
    }

    #[test]
    pub fn test_is_googler_host_domain() {
        assert_eq!(true, is_googler_host_domain("c.googlers.com"));
        assert_eq!(true, is_googler_host_domain("corp.google.com"));
        assert_eq!(true, is_googler_host_domain("ldpa1.c.googlers.com"));
        assert_eq!(true, is_googler_host_domain("ldap2.mtv.corp.google.com"));
    }

    #[test]
    pub fn test_not_is_googler_host_domain() {
        assert_eq!(false, is_googler_host_domain(""));
        assert_eq!(false, is_googler_host_domain("c"));
        assert_eq!(false, is_googler_host_domain("c.googlers"));
        assert_eq!(false, is_googler_host_domain("google.com"));
        assert_eq!(false, is_googler_host_domain("c.googlers.com.edu"));
        assert_eq!(false, is_googler_host_domain("corp.google.com.edu"));
        assert_eq!(false, is_googler_host_domain("ldpa1.c.googlers.com.edu"));
    }

    #[test]
    pub fn test_redact_user_value() {
        let parameter = "hello player1";
        let result = redact_user_name_from(parameter);
        assert_eq!("hello $USER", result);
    }

    #[test]
    pub fn test_redact_user_value_no_match() {
        let parameter = "hello world";
        let result = redact_user_name_from(parameter);
        assert_eq!("hello world", result);
    }

    #[test]
    pub fn test_new_analytics_folder() {
        let expected = "/tmp/testinganalytics/.local/share/Fuchsia/metrics/";
        let result = new_analytics_folder();
        assert_eq!(Some(expected), result.to_str());
    }

    #[test]
    #[ignore]
    pub fn test_copy_old_to_new() {
        let _ = remove_dir_all(PathBuf::from("/tmp/testinganalytics"));
        let old_analytics_folder = old_analytics_folder();
        let _ = create_dir_all(&old_analytics_folder);
        let uuid_path = &old_analytics_folder.join("uuid");
        let _ = write(&uuid_path, "12345");

        let new_analytics_folder = new_analytics_folder();
        let _ = create_dir_all(&new_analytics_folder);

        assert_eq!(
            true,
            (&old_analytics_folder).exists(),
            "expected old analytics folder to exist"
        );

        assert_eq!(
            true,
            (&new_analytics_folder).exists(),
            "expected new analytics folder to exist"
        );

        let _result = copy_old_to_new(&old_analytics_folder, &new_analytics_folder).unwrap();

        let new_uuid_path = &new_analytics_folder.join("uuid");
        let msg = format!("Expected new copy of file to exist: {:?}", &new_uuid_path.as_path());
        assert_eq!(true, (&new_uuid_path).exists(), "{}", msg);
        let _ = remove_dir_all(PathBuf::from("/tmp/testinganalytics"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore]
    pub fn test_symlink_old_to_new() {
        let _ = remove_dir_all(PathBuf::from("/tmp/testinganalytics"));
        let old_analytics_folder = old_analytics_folder();
        if old_analytics_folder.exists() {
            let _ = remove_dir_all(&old_analytics_folder);
        }

        let new_analytics_folder = new_analytics_folder();
        let _ = create_dir_all(&new_analytics_folder);

        assert_eq!(true, new_analytics_folder.exists());
        assert_eq!(false, old_analytics_folder.exists());

        symlink_old_to_new(&new_analytics_folder, &old_analytics_folder);

        assert_eq!(true, old_analytics_folder.exists());

        let metadata = symlink_metadata(&old_analytics_folder).unwrap();
        assert_eq!(true, metadata.is_symlink());
        assert_eq!(&new_analytics_folder, &read_link(&old_analytics_folder).unwrap());
        let _ = remove_dir_all(PathBuf::from("/tmp/testinganalytics"));
    }
}
