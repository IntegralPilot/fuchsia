// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::AnalyticsError;
use std::collections::{BTreeMap, HashMap};

use crate::analytics_client::GA4AnalyticsClient;
use crate::env_info::{get_arch, get_os, is_googler};
use crate::ga4_event::*;
use crate::metrics_state::*;
use crate::notice::{BRIEF_NOTICE, FULL_NOTICE, GOOGLER_ENHANCED_NOTICE, SHOW_NOTICE_TEMPLATE};

/// The implementation of the GA4 Measurement Protocol metrics public api.
//#[derive(Clone)]
pub struct GA4MetricsService {
    metrics_state: MetricsState,
    client: Option<GA4AnalyticsClient>,
    post: Post,
}

impl GA4MetricsService {
    pub(crate) fn new(state: MetricsState) -> Self {
        let client = if state.status.is_opted_in() {
            Some(GA4AnalyticsClient::new(state.ga4_key.clone(), state.ga4_product_code.clone()))
        } else {
            None
        };
        let mut svc = GA4MetricsService { metrics_state: state, client, post: Post::default() };
        svc.init_post();
        svc
    }

    /// Returns Analytics disclosure notice according to PDD rules.
    pub fn get_notice(&self) -> Option<String> {
        if !is_googler() {
            match self.metrics_state.status {
                MetricsStatus::NewUser => Some(FULL_NOTICE.to_string()),
                MetricsStatus::NewToTool => Some(BRIEF_NOTICE.to_string()),
                _ => None,
            }
        } else {
            match self.metrics_state.status {
                MetricsStatus::GooglerNeedsNotice | MetricsStatus::GooglerOptedInAndNeedsNotice => {
                    if let Some(invoker) = &self.metrics_state.invoker {
                        if invoker != "fx" {
                            Some(GOOGLER_ENHANCED_NOTICE.to_string())
                        } else {
                            None
                        }
                    } else {
                        Some(GOOGLER_ENHANCED_NOTICE.to_string())
                    }
                }
                _ => None,
            }
        }
    }

    pub async fn show_status_message(&self) -> String {
        let optin_status = match self.metrics_state.status {
            MetricsStatus::OptedIn
            | MetricsStatus::GooglerOptedInAndNeedsNotice
            | MetricsStatus::NewToTool => "enabled",
            MetricsStatus::OptedInEnhanced => "enable-enhanced",
            MetricsStatus::OptedOut
            | MetricsStatus::Disabled
            | MetricsStatus::NewUser
            | MetricsStatus::GooglerNeedsNotice => "disabled",
        };
        let message = SHOW_NOTICE_TEMPLATE.replace("{status}", optin_status);
        message
    }

    /// Records Analytics participation status.
    /// TODO remove this once foxtrot is migrated to set_new_opt_in_status
    pub fn set_opt_in_status(&mut self, enabled: bool) -> Result<(), AnalyticsError> {
        if enabled {
            if self.client.is_none() {
                self.client = Some(GA4AnalyticsClient::new(
                    self.metrics_state.ga4_key.clone(),
                    self.metrics_state.ga4_product_code.clone(),
                ));
            }
        } else {
            self.client = None;
        }
        self.metrics_state.set_opt_in_status(enabled)
    }

    /// Record analytics participation status in new migrated status file to support
    /// enhanced analytics for Googlers.
    pub fn set_new_opt_in_status(&mut self, status: MetricsStatus) -> Result<(), AnalyticsError> {
        if status.is_opted_in() {
            if self.client.is_none() {
                self.client = Some(GA4AnalyticsClient::new(
                    self.metrics_state.ga4_key.clone(),
                    self.metrics_state.ga4_product_code.clone(),
                ));
            }
        } else {
            self.client = None;
        }
        self.metrics_state.set_new_opt_in_status(status)
    }

    pub fn opt_in_status(&self) -> MetricsStatus {
        self.metrics_state.status.clone()
    }

    /// Returns Analytics participation status.
    pub fn is_opted_in(&self) -> bool {
        self.metrics_state.status.is_opted_in()
    }

    /// Disables analytics for this invocation only.
    /// This does not affect the global analytics state.
    pub fn opt_out_for_this_invocation(&mut self) -> Result<(), AnalyticsError> {
        self.client = None;
        self.metrics_state.opt_out_for_this_invocation()
    }

    /// Adds a launch event to the Post
    pub async fn add_launch_event(&mut self, args: Option<&str>) -> Result<(), AnalyticsError> {
        self.add_custom_event(None, args, args, BTreeMap::new(), Some("launch")).await
    }

    /// Adds an event to the post with open-ended parameters
    /// while still honoring the UA Event parameters already
    /// in use.
    pub async fn add_custom_event(
        &mut self,
        category: Option<&str>,
        action: Option<&str>,
        label: Option<&str>,
        custom_dimensions: BTreeMap<&str, GA4Value>,
        event_name: Option<&str>,
    ) -> Result<(), AnalyticsError> {
        if !self.is_opted_in() {
            return Ok(());
        }
        let ga4_event = make_ga4_event(
            category,
            action,
            label,
            custom_dimensions,
            self.metrics_state.invoker.as_deref(),
            event_name,
        );
        self.post.add_event(ga4_event);
        Ok(())
    }

    /// Adds a crash/exception event to the post
    /// conforming to the UA Event parameters already
    /// in use.
    // TODO With GA4's flexibility, rework exception reporting to be more informative
    pub async fn add_crash_event(
        &mut self,
        description: &str,
        fatal: Option<&bool>,
    ) -> Result<(), AnalyticsError> {
        if !self.is_opted_in() {
            return Ok(());
        }
        let ga4_event =
            make_ga4_crash_event(description, fatal, self.metrics_state.invoker.as_deref());
        self.post.add_event(ga4_event);
        Ok(())
    }

    /// Records a timing event from the app.
    pub async fn add_timing_event(
        &mut self,
        category: Option<&str>,
        time: u64,
        variable: Option<&str>,
        label: Option<&str>,
        custom_dimensions: BTreeMap<&str, GA4Value>,
    ) -> Result<(), AnalyticsError> {
        if !self.is_opted_in() {
            return Ok(());
        }
        let ga4_event = make_ga4_timing_event(
            category,
            time,
            variable,
            label,
            custom_dimensions,
            self.metrics_state.invoker.as_deref(),
        );
        self.post.add_event(ga4_event);
        Ok(())
    }

    /// Sends the Post, with all accumulated events
    /// to the Google Analytics service.
    pub async fn send_events(&mut self) -> Result<(), AnalyticsError> {
        if !self.is_opted_in() {
            return Ok(());
        }
        self.rewrite_ua_ffx_known_batch_to_ga4();

        if let Some(ref client) = self.client {
            let _ = self.post.validate()?;
            client.send(&mut self.post).await
        } else {
            // Normally analytics errors are logged as traces only, esp. those caused by network
            // errors. However this branch being reachable would be a real bug in the code.
            // We log the error and disable analytics for the session.
            log::error!("Analytics Error: branch should not be reachable");
            // Unwrap is safe here as the function only returns Ok(()).
            self.opt_out_for_this_invocation().unwrap();
            Ok(())
        }
    }

    // Send a signal analytics only if the user is a new internal user.
    pub(crate) async fn send_signal_if_new_internal_user(&self) {
        if !self.metrics_state.is_new_internal_user {
            return;
        }
        let Some(uuid) = &self.metrics_state.uuid else {
            log::warn!("UUID is missing when sending signal for new internal user");
            return;
        };
        // For a new user, a full functional metrics service is not ready yet.
        // We need to send the signal with more low level functions.
        let client = GA4AnalyticsClient::new(
            self.metrics_state.ga4_key.clone(),
            self.metrics_state.ga4_product_code.clone(),
        );
        let mut post = Post::new(
            "new_user".into(),
            None,
            None,
            vec![Event::new(
                "new_user".into(),
                Some(Params {
                    items: None,
                    params: HashMap::from([("uuid".to_owned(), uuid.to_string().into())]),
                }),
            )],
        );
        let _ = client.send(&mut post).await;
    }

    /// Rewrites the batch call from ffx invoke under UA analytics
    /// to a single event under GA4 Analytics.
    /// TODO Remove this once we remove UA analtyics and the ffx client has been updated
    /// to speak to the GA4 Metrics Service.
    fn rewrite_ua_ffx_known_batch_to_ga4(&mut self) {
        if self.post.events.len() == 2
            && self.post.events[0].name.eq_ignore_ascii_case("invoke")
            && self.post.events[1].name.eq_ignore_ascii_case("timing")
        {
            log::trace!("Rewriting ffx batch invoke to ga4 post invoke");
            let events = &mut self.post.events;
            let invoke_event = &mut events[0].clone();
            let timing_event = events.remove(1);
            if let Some(params) = timing_event.params {
                if params.params.contains_key("time") {
                    let time = params.params["time"].clone();
                    invoke_event.add_param("timing", time);
                }
                self.post.events = vec![invoke_event.to_owned()];
            }
        }
    }

    fn uuid_as_str(&self) -> String {
        self.metrics_state.uuid.map_or_else(|| "No uuid".to_string(), |u| u.to_string())
    }

    /// Create the GA4 Post object that will be sent to Google Analytics.
    fn init_post(&mut self) {
        self.post = Post::new(self.uuid_as_str(), None, Some(self.make_user_properties()), vec![]);
    }

    /// Initialize the UserProperties to be sent to GA4 with events.
    fn make_user_properties(&self) -> HashMap<String, ValueObject> {
        HashMap::from([
            (
                "build_version".into(),
                ValueObject { value: self.metrics_state.build_version.clone().into() },
            ),
            ("os".into(), ValueObject { value: get_os().into() }),
            ("arch".into(), ValueObject { value: get_arch().into() }),
            (
                "sdk_version".into(),
                ValueObject { value: self.metrics_state.sdk_version.clone().into() },
            ),
            ("internal".into(), ValueObject { value: is_googler_as_int().into() }),
            ("metrics_level".into(), ValueObject { value: self.opted_in_metrics_level().into() }),
        ])
    }

    // This returns which level of opted in is set
    // only when user is opted in.
    // Used to encode level for analytics.
    fn opted_in_metrics_level(&self) -> u64 {
        if self.opt_in_status() == MetricsStatus::OptedInEnhanced { 2 } else { 1 }
    }
}

// encode bool as 1 or 0 for analytics
fn is_googler_as_int() -> u64 {
    match is_googler() {
        true => 1,
        false => 0,
    }
}

impl Default for GA4MetricsService {
    fn default() -> Self {
        let metrics_state = MetricsState::default();
        let client = Some(GA4AnalyticsClient::new(
            metrics_state.ga4_key.clone(),
            metrics_state.ga4_product_code.clone(),
        ));
        Self { metrics_state, client, post: Post::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    const APP_NAME: &str = "my cool app";
    const BUILD_VERSION: &str = "12/09/20 00:00:00";
    const SDK_VERSION: &str = "99.99.99.99.1";
    // const LAUNCH_ARGS: &str = "config analytics enable";

    fn test_metrics_svc(
        app_support_dir_path: &PathBuf,
        app_name: String,
        build_version: String,
        sdk_version: String,
        ga_product_code: String,
        ga4_product_code: String,
        ga4_key: String,
        disabled: bool,
    ) -> GA4MetricsService {
        GA4MetricsService::new(MetricsState::from_config(
            app_support_dir_path,
            app_name,
            build_version,
            sdk_version,
            ga_product_code,
            ga4_product_code,
            ga4_key,
            disabled,
            None,
        ))
    }

    #[test]
    fn new_user_of_any_tool() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let dir = create_tmp_metrics_dir()?;
        let ms = test_metrics_svc(
            &dir,
            String::from(APP_NAME),
            String::from(BUILD_VERSION),
            String::from(SDK_VERSION),
            UNKNOWN_PROPERTY_ID.to_string(),
            UNKNOWN_GA4_PRODUCT_CODE.to_string(),
            UNKNOWN_GA4_KEY.to_string(),
            false,
        );

        if !is_googler() {
            assert_eq!(ms.get_notice(), Some(FULL_NOTICE.into()));
        } else {
            assert_eq!(ms.get_notice(), Some(GOOGLER_ENHANCED_NOTICE.into()));
        }

        drop(dir);
        Ok(())
    }

    #[test]
    fn existing_user_first_use_of_this_tool() -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    {
        let dir = create_tmp_metrics_dir()?;
        write_opt_in_status(&dir, true)?;

        let ms = test_metrics_svc(
            &dir,
            String::from(APP_NAME),
            String::from(BUILD_VERSION),
            String::from(SDK_VERSION),
            UNKNOWN_PROPERTY_ID.to_string(),
            UNKNOWN_GA4_PRODUCT_CODE.to_string(),
            UNKNOWN_GA4_KEY.to_string(),
            false,
        );
        if !is_googler() {
            assert_eq!(ms.metrics_state.status, MetricsStatus::NewToTool);
        } else {
            assert_eq!(ms.metrics_state.status, MetricsStatus::GooglerOptedInAndNeedsNotice);
        }
        if !is_googler() {
            assert_eq!(ms.get_notice(), Some(BRIEF_NOTICE.into()));
        } else {
            assert_eq!(ms.get_notice(), Some(GOOGLER_ENHANCED_NOTICE.into()));
        }
        drop(dir);
        Ok(())
    }

    #[test]
    fn existing_user_of_this_tool_opted_in() -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    {
        let dir = create_tmp_metrics_dir()?;
        write_opt_in_status(&dir, true)?;
        write_app_status(&dir, &APP_NAME, true)?;
        let ms = test_metrics_svc(
            &dir,
            String::from(APP_NAME),
            String::from(BUILD_VERSION),
            String::from(SDK_VERSION),
            UNKNOWN_PROPERTY_ID.to_string(),
            UNKNOWN_GA4_PRODUCT_CODE.to_string(),
            UNKNOWN_GA4_KEY.to_string(),
            false,
        );

        if !is_googler() {
            assert_eq!(ms.get_notice(), None);
        } else {
            assert_eq!(ms.get_notice(), Some(GOOGLER_ENHANCED_NOTICE.into()));
        }
        drop(dir);
        Ok(())
    }

    #[test]
    fn existing_user_of_this_tool_opted_out() -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    {
        let dir = create_tmp_metrics_dir()?;
        write_opt_in_status(&dir, false)?;
        write_app_status(&dir, &APP_NAME, true)?;
        let ms = test_metrics_svc(
            &dir,
            String::from(APP_NAME),
            String::from(BUILD_VERSION),
            String::from(SDK_VERSION),
            UNKNOWN_PROPERTY_ID.to_string(),
            UNKNOWN_GA4_PRODUCT_CODE.to_string(),
            UNKNOWN_GA4_KEY.to_string(),
            false,
        );

        assert_eq!(ms.get_notice(), None);

        drop(dir);
        Ok(())
    }

    #[test]
    fn with_disable_env_var_set() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let dir = create_tmp_metrics_dir()?;
        write_opt_in_status(&dir, true)?;
        write_app_status(&dir, &APP_NAME, true)?;

        let ms = test_metrics_svc(
            &dir,
            String::from(APP_NAME),
            String::from(BUILD_VERSION),
            String::from(SDK_VERSION),
            UNKNOWN_PROPERTY_ID.to_string(),
            UNKNOWN_GA4_PRODUCT_CODE.to_string(),
            UNKNOWN_GA4_KEY.to_string(),
            true,
        );

        assert_eq!(ms.get_notice(), None);

        drop(dir);
        Ok(())
    }

    #[test]
    fn opt_out_for_this_invocation() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let dir = create_tmp_metrics_dir()?;
        let mut ms = test_metrics_svc(
            &dir,
            String::from(APP_NAME),
            String::from(BUILD_VERSION),
            String::from(SDK_VERSION),
            UNKNOWN_PROPERTY_ID.to_string(),
            UNKNOWN_GA4_PRODUCT_CODE.to_string(),
            UNKNOWN_GA4_KEY.to_string(),
            false,
        );

        if !is_googler() {
            assert_eq!(ms.metrics_state.status, MetricsStatus::NewUser);
        } else {
            assert_eq!(ms.metrics_state.status, MetricsStatus::GooglerNeedsNotice);
        }
        let _res = ms.opt_out_for_this_invocation().unwrap();
        assert_eq!(ms.metrics_state.status, MetricsStatus::OptedOut);

        drop(dir);
        Ok(())
    }

    #[test]
    fn opt_in_from_opted_out() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let dir = create_tmp_metrics_dir()?;
        write_opt_in_status(&dir, false)?;
        let mut ms = test_metrics_svc(
            &dir,
            String::from(APP_NAME),
            String::from(BUILD_VERSION),
            String::from(SDK_VERSION),
            UNKNOWN_PROPERTY_ID.to_string(),
            UNKNOWN_GA4_PRODUCT_CODE.to_string(),
            UNKNOWN_GA4_KEY.to_string(),
            false,
        );
        assert!(!ms.metrics_state.status.is_opted_in());
        assert!(ms.client.is_none());
        ms.set_new_opt_in_status(MetricsStatus::OptedIn).unwrap();
        assert!(ms.metrics_state.status.is_opted_in(), "{:?}", ms.metrics_state.status);
        assert!(ms.client.is_some());

        drop(dir);
        Ok(())
    }

    #[test]
    fn opt_out_from_opted_in() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let dir = create_tmp_metrics_dir()?;
        write_opt_in_status(&dir, true)?;
        let mut ms = test_metrics_svc(
            &dir,
            String::from(APP_NAME),
            String::from(BUILD_VERSION),
            String::from(SDK_VERSION),
            UNKNOWN_PROPERTY_ID.to_string(),
            UNKNOWN_GA4_PRODUCT_CODE.to_string(),
            UNKNOWN_GA4_KEY.to_string(),
            false,
        );

        assert!(ms.metrics_state.status.is_opted_in(), "{:?}", ms.metrics_state.status);
        assert!(ms.client.is_some());
        ms.set_new_opt_in_status(MetricsStatus::OptedOut).unwrap();
        assert!(!ms.metrics_state.status.is_opted_in());
        assert!(ms.client.is_none());

        drop(dir);
        Ok(())
    }

    #[test]
    fn opt_in_from_opted_in() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let dir = create_tmp_metrics_dir()?;
        write_opt_in_status(&dir, true)?;
        let mut ms = test_metrics_svc(
            &dir,
            String::from(APP_NAME),
            String::from(BUILD_VERSION),
            String::from(SDK_VERSION),
            UNKNOWN_PROPERTY_ID.to_string(),
            UNKNOWN_GA4_PRODUCT_CODE.to_string(),
            UNKNOWN_GA4_KEY.to_string(),
            false,
        );

        assert!(ms.metrics_state.status.is_opted_in(), "{:?}", ms.metrics_state.status);
        assert!(ms.client.is_some());
        ms.set_new_opt_in_status(MetricsStatus::OptedIn).unwrap();
        assert!(ms.metrics_state.status.is_opted_in());
        assert!(ms.client.is_some());

        drop(dir);
        Ok(())
    }

    #[test]
    fn opt_out_from_opted_out() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let dir = create_tmp_metrics_dir()?;
        write_opt_in_status(&dir, false)?;
        let mut ms = test_metrics_svc(
            &dir,
            String::from(APP_NAME),
            String::from(BUILD_VERSION),
            String::from(SDK_VERSION),
            UNKNOWN_PROPERTY_ID.to_string(),
            UNKNOWN_GA4_PRODUCT_CODE.to_string(),
            UNKNOWN_GA4_KEY.to_string(),
            false,
        );

        assert!(!ms.metrics_state.status.is_opted_in(), "{:?}", ms.metrics_state.status);
        assert!(ms.client.is_none());
        ms.set_new_opt_in_status(MetricsStatus::OptedOut).unwrap();
        assert!(!ms.metrics_state.status.is_opted_in());
        assert!(ms.client.is_none());

        drop(dir);
        Ok(())
    }

    pub fn create_tmp_metrics_dir() -> Result<PathBuf, AnalyticsError> {
        let tmp_dir = tempdir()?;
        let dir_obj = tmp_dir.path().join("fuchsia_metrics");
        let dir = dir_obj.as_path();
        Ok(dir.to_owned())
    }
}
