// Copyright 2024 The Fuchsia Authors
//
// Licensed under a BSD-style license <LICENSE-BSD>, Apache License, Version 2.0
// <LICENSE-APACHE or https://www.apache.org/licenses/LICENSE-2.0>, or the MIT
// license <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your option.
// This file may not be copied, modified, or distributed except according to
// those terms.

// This is a minimal implementation to satisfy the state machine builder.
// It is roughly identical to
// https://cs.opensource.google/fuchsia/fuchsia/+/main:src/sys/pkg/lib/omaha-client/src/installer/stub.rs

use futures::{future::LocalBoxFuture, prelude::*};
use omaha_client::cup_ecdsa::RequestMetadata;
use omaha_client::installer::{AppInstallResult, Installer, Plan, ProgressObserver};
use omaha_client::protocol::response::Response;
use omaha_client::request_builder::RequestParams;
use thiserror::Error;

/// This is the collection of Errors that can occur during the installation of an update.
///
#[derive(Debug, Error, Eq, PartialEq)]
pub enum MinimalInstallErrors {
    #[error("Minimal Installer Failure")]
    Failed,
}

/// A minimal implementation of the Install Result.
pub enum MinimalInstallResult {
    Installed,
    Failed,
}

/// A minimal implementation of the Install Plan.
///
pub struct MinimalInstallPlan;

impl Plan for MinimalInstallPlan {
    fn id(&self) -> String {
        String::new()
    }
}

/// The Installer is responsible for performing (or triggering) the installation of the update
/// that's referred to by the InstallPlan.
///
#[derive(Debug, Default)]
pub struct MinimalInstaller {
    pub should_fail: bool,
}

impl Installer for MinimalInstaller {
    type InstallPlan = MinimalInstallPlan;
    type Error = MinimalInstallErrors;
    type InstallResult = MinimalInstallResult;

    /// Perform the installation as given by the install plan (as parsed form the Omaha server
    /// response).  If given, provide progress via the observer, and a final finished or Error
    /// indication via the Future.
    fn perform_install(
        &mut self,
        _install_plan: &MinimalInstallPlan,
        _observer: Option<&dyn ProgressObserver>,
    ) -> LocalBoxFuture<'_, (Self::InstallResult, Vec<AppInstallResult<Self::Error>>)> {
        if self.should_fail {
            future::ready((
                MinimalInstallResult::Failed,
                vec![AppInstallResult::Failed(MinimalInstallErrors::Failed)],
            ))
            .boxed_local()
        } else {
            future::ready((MinimalInstallResult::Installed, vec![AppInstallResult::Installed]))
                .boxed_local()
        }
    }

    fn perform_reboot(&mut self) -> LocalBoxFuture<'_, Result<(), anyhow::Error>> {
        future::ready(Ok(())).boxed_local()
    }

    fn try_create_install_plan<'a>(
        &'a self,
        _request_params: &'a RequestParams,
        _request_metadata: Option<&'a RequestMetadata>,
        response: &'a Response,
        _response_bytes: Vec<u8>,
        _ecdsa_signature: Option<Vec<u8>>,
    ) -> LocalBoxFuture<'a, Result<Self::InstallPlan, Self::Error>> {
        if response.protocol_version != "3.0" {
            future::ready(Err(MinimalInstallErrors::Failed)).boxed_local()
        } else {
            future::ready(Ok(MinimalInstallPlan)).boxed_local()
        }
    }
}
