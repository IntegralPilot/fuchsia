// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_test_manager::LaunchError;
use fuchsia_component_test::error::Error as RealmBuilderError;
use log::warn;
use thiserror::Error;
use {fidl_fuchsia_component as fcomponent, fidl_fuchsia_debugger as fdbg};

/// Error encountered running test manager
#[derive(Debug, Error)]
pub enum TestManagerError {
    #[error("Error sending response: {0:?}")]
    Response(#[source] fidl::Error),

    #[error("Error serving test manager protocol: {0:?}")]
    Stream(#[source] fidl::Error),

    #[error("Cannot convert to request stream: {0:?}")]
    IntoStream(#[source] fidl::Error),
}

#[derive(Debug, Error)]
pub enum LaunchTestError {
    #[error("Failed to initialize test realm: {0:?}")]
    InitializeTestRealm(#[source] RealmBuilderError),

    #[error("Failed to create test realm: {0:?}")]
    CreateTestRealm(#[source] RealmBuilderError),

    #[error("Failed to create test: {0:?}")]
    CreateTest(fcomponent::Error),

    #[error("Failed to create test: {0:?}")]
    CreateTestFidl(fidl::Error),

    #[error("Failed to connect to embedded ArchiveAccessor: {0:?}")]
    ConnectToArchiveAccessor(#[source] anyhow::Error),

    #[error("Failed to connect to embedded LogSettings: {0:?}")]
    ConnectToLogSettings(#[source] anyhow::Error),

    #[error("Failed to set log interest with the embedded LogSettings: {0:?}")]
    SetLogInterest(#[source] anyhow::Error),

    #[error("Failed to connect to TestSuite: {0:?}")]
    ConnectToTestSuite(#[source] anyhow::Error),

    #[error("Failed to connect to StorageAdmin: {0:?}")]
    ConnectToStorageAdmin(#[source] anyhow::Error),

    #[error("Cannot open exposed directory: {0:?}")]
    OpenExposedDir(#[source] anyhow::Error),

    #[error("Failed to stream logs from embedded Archivist: {0:?}")]
    StreamIsolatedLogs(anyhow::Error),

    #[error("Failed to resolve test: {0:?}")]
    ResolveTest(#[source] anyhow::Error),

    #[error("Failed to read manifest: {0}")]
    ManifestIo(mem_util::DataError),

    #[error("Resolver returned invalid manifest data")]
    InvalidResolverData,

    #[error("Invalid manifest: {0:?}")]
    InvalidManifest(#[source] anyhow::Error),

    #[error("Failed validating test realm: {0:?}")]
    ValidateTestRealm(#[source] anyhow::Error),
}

#[derive(Debug, Error)]
pub enum DebugAgentError {
    #[error("Failed to connect to DebugAgent: {0:?}")]
    ConnectToLauncher(#[source] anyhow::Error),

    #[error("Launcher::Launch (for DebugAgent) failed with local error: {0:?}")]
    LaunchLocal(#[source] fidl::Error),

    #[error("Launcher::Launch (for DebugAgent) produced error response: {0:?}")]
    LaunchResponse(i32),

    #[error("DebugAgent::AttachTo failed with local error: {0:?}")]
    AttachToTestsLocal(#[source] fidl::Error),

    #[error("DebugAgent::AttachTo produced error response: {0:?}")]
    AttachToTestsResponse(fdbg::FilterError),
}

#[derive(Debug, Error)]
pub enum FacetError {
    #[error("Facet '{0}' defined but is null")]
    NullFacet(&'static str),

    #[error("Invalid facet: {0}, value: {1:?}, allowed value(s): {2}")]
    InvalidFacetValue(&'static str, String, String),
}

impl From<FacetError> for LaunchTestError {
    fn from(e: FacetError) -> Self {
        Self::InvalidManifest(e.into())
    }
}

impl From<LaunchTestError> for LaunchError {
    fn from(e: LaunchTestError) -> Self {
        // log the error so that we don't lose it while converting to
        // fidl equivalent.
        // TODO(https://fxbug.dev/42057092): remove this warning.
        warn!("Error launching test: {:?}", e);
        match e {
            LaunchTestError::InitializeTestRealm(_)
            | LaunchTestError::ValidateTestRealm(_)
            | LaunchTestError::ConnectToArchiveAccessor(_)
            | LaunchTestError::ConnectToLogSettings(_)
            | LaunchTestError::SetLogInterest(_)
            | LaunchTestError::CreateTestFidl(_)
            | LaunchTestError::CreateTest(_)
            | LaunchTestError::StreamIsolatedLogs(_)
            | LaunchTestError::OpenExposedDir(_)
            | LaunchTestError::ConnectToStorageAdmin(_) => Self::InternalError,
            LaunchTestError::InvalidResolverData
            | LaunchTestError::InvalidManifest(_)
            | LaunchTestError::ManifestIo(_) => Self::InvalidManifest,
            LaunchTestError::CreateTestRealm(_) | LaunchTestError::ResolveTest(_) => {
                Self::InstanceCannotResolve
            }
            LaunchTestError::ConnectToTestSuite(_) => Self::FailedToConnectToTestSuite,
        }
    }
}
