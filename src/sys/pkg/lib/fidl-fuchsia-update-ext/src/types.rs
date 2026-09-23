// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use event_queue::Event;
use fidl_fuchsia_update_common as fidl;
use proptest::prelude::*;
use proptest_derive::Arbitrary;
use std::fmt;
use thiserror::Error;
use typed_builder::TypedBuilder;

/// Wrapper type for [`fidl_fuchsia_update::State`] which works with
/// [`event_queue`] and [`proptest`].
///
/// Use [`From`] (and [`Into`]) to convert between the fidl type and this one.
///
/// See [`fidl_fuchsia_update::State`] for docs on what each state means.
#[allow(missing_docs)] // states are documented in fidl.
#[derive(Clone, Debug, PartialEq, Arbitrary)]
pub enum State {
    CheckingForUpdates,
    ErrorCheckingForUpdate,
    NoUpdateAvailable,
    InstallationDeferredByPolicy(InstallationDeferredData),
    InstallingUpdate(InstallingData),
    WaitingForReboot(InstallingData),
    InstallationError(InstallationErrorData),
}

impl State {
    /// Returns true if this state is an error state.
    pub fn is_error(&self) -> bool {
        match self {
            State::ErrorCheckingForUpdate | State::InstallationError(_) => true,
            State::CheckingForUpdates
            | State::NoUpdateAvailable
            | State::InstallationDeferredByPolicy(_)
            | State::InstallingUpdate(_)
            | State::WaitingForReboot(_) => false,
        }
    }

    /// Returns true if this state is a terminal state.
    pub fn is_terminal(&self) -> bool {
        match self {
            State::CheckingForUpdates | State::InstallingUpdate(_) => false,
            State::ErrorCheckingForUpdate
            | State::InstallationError(_)
            | State::NoUpdateAvailable
            | State::InstallationDeferredByPolicy(_)
            | State::WaitingForReboot(_) => true,
        }
    }
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            State::CheckingForUpdates => write!(f, "Checking for updates...")?,
            State::ErrorCheckingForUpdate => write!(f, "Error checking for update!")?,
            State::InstallationError(_) => write!(f, "Installation error: {self:?}")?,
            State::NoUpdateAvailable => write!(f, "No update available.")?,
            State::InstallationDeferredByPolicy(InstallationDeferredData {
                deferral_reason,
                ..
            }) => {
                write!(f, "Installation deferred by policy: {deferral_reason:?}")?;
            }
            State::InstallingUpdate(InstallingData { update, installation_progress }) => {
                if let Some(UpdateInfo { version_available: Some(version), .. }) = update {
                    write!(f, "Installing {version}")?;
                } else {
                    write!(f, "Installing update")?;
                }
                if let Some(InstallationProgress { fraction_completed: Some(fraction) }) =
                    installation_progress
                {
                    write!(f, " ({:.2}%)", fraction * 100.0)?;
                }
            }
            State::WaitingForReboot(_) => write!(f, "Waiting for reboot...")?,
        }
        Ok(())
    }
}

impl Event for State {
    fn can_merge(&self, other: &State) -> bool {
        if self == other {
            return true;
        }
        // Merge states that have the same update info but different installation
        // progress
        if let State::InstallingUpdate(InstallingData { update: update0, .. }) = self
            && let State::InstallingUpdate(InstallingData { update: update1, .. }) = other
        {
            return update0 == update1;
        }
        false
    }
}

impl From<State> for fidl::State {
    fn from(other: State) -> Self {
        match other {
            State::CheckingForUpdates => {
                fidl::State::CheckingForUpdates(fidl::CheckingForUpdatesData::default())
            }
            State::ErrorCheckingForUpdate => {
                fidl::State::ErrorCheckingForUpdate(fidl::ErrorCheckingForUpdateData::default())
            }
            State::NoUpdateAvailable => {
                fidl::State::NoUpdateAvailable(fidl::NoUpdateAvailableData::default())
            }
            State::InstallationDeferredByPolicy(data) => {
                fidl::State::InstallationDeferredByPolicy(data.into())
            }
            State::InstallingUpdate(data) => fidl::State::InstallingUpdate(data.into()),
            State::WaitingForReboot(data) => fidl::State::WaitingForReboot(data.into()),
            State::InstallationError(data) => fidl::State::InstallationError(data.into()),
        }
    }
}

impl From<fidl::State> for State {
    fn from(fidl_state: fidl::State) -> Self {
        match fidl_state {
            fidl::State::CheckingForUpdates(_) => State::CheckingForUpdates,
            fidl::State::ErrorCheckingForUpdate(_) => State::ErrorCheckingForUpdate,
            fidl::State::NoUpdateAvailable(_) => State::NoUpdateAvailable,
            fidl::State::InstallationDeferredByPolicy(data) => {
                State::InstallationDeferredByPolicy(data.into())
            }
            fidl::State::InstallingUpdate(data) => State::InstallingUpdate(data.into()),
            fidl::State::WaitingForReboot(data) => State::WaitingForReboot(data.into()),
            fidl::State::InstallationError(data) => State::InstallationError(data.into()),
        }
    }
}

/// An error which can be returned when validating [`AttemptOptions`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AttemptOptionsDecodeError {
    /// The initiator field was not set.
    #[error("missing field 'initiator'")]
    MissingInitiator,
}

/// Wrapper type for [`fidl_fuchsia_update::AttemptOptions`] which validates the
/// options on construction and works with [`proptest`].
///
/// Use [`TryFrom`] (or [`TryInto`]) to convert the fidl type and this one, and
/// [`From`] (or [`Into`]) to convert back to the fidl type.
#[derive(Clone, Debug, PartialEq, Arbitrary)]
pub struct AttemptOptions {
    pub initiator: Initiator,
}

impl Event for AttemptOptions {
    fn can_merge(&self, _other: &AttemptOptions) -> bool {
        true
    }
}

impl TryFrom<fidl::AttemptOptions> for AttemptOptions {
    type Error = AttemptOptionsDecodeError;

    fn try_from(o: fidl::AttemptOptions) -> Result<Self, Self::Error> {
        Ok(Self {
            initiator: o.initiator.ok_or(AttemptOptionsDecodeError::MissingInitiator)?.into(),
        })
    }
}

impl From<AttemptOptions> for fidl::AttemptOptions {
    fn from(o: AttemptOptions) -> Self {
        Self { initiator: Some(o.initiator.into()), ..Default::default() }
    }
}

impl From<CheckOptions> for AttemptOptions {
    fn from(o: CheckOptions) -> Self {
        Self { initiator: o.initiator }
    }
}

/// Wrapper type for [`fidl_fuchsia_update::InstallationErrorData`] which works
/// with [`proptest`].
///
/// Use [`From`] (and [`Into`]) to convert between the fidl type and this one.
#[derive(Clone, Debug, Default, PartialEq, Arbitrary)]
pub struct InstallationErrorData {
    pub update: Option<UpdateInfo>,
    pub installation_progress: Option<InstallationProgress>,
}
impl From<InstallationErrorData> for fidl::InstallationErrorData {
    fn from(other: InstallationErrorData) -> Self {
        fidl::InstallationErrorData {
            update: other.update.map(|ext| ext.into()),
            installation_progress: other.installation_progress.map(|ext| ext.into()),
            ..Default::default()
        }
    }
}
impl From<fidl::InstallationErrorData> for InstallationErrorData {
    fn from(data: fidl::InstallationErrorData) -> Self {
        Self {
            update: data.update.map(|o| o.into()),
            installation_progress: data.installation_progress.map(|o| o.into()),
        }
    }
}

/// Wrapper type for [`fidl_fuchsia_update::InstallationProgress`] which works
/// with [`proptest`].
///
/// Use [`From`] (and [`Into`]) to convert between the fidl type and this one.
#[derive(Clone, Debug, PartialEq, Arbitrary)]
pub struct InstallationProgress {
    #[proptest(strategy = "prop::option::of(prop::num::f32::NORMAL)")]
    pub fraction_completed: Option<f32>,
}
impl From<InstallationProgress> for fidl::InstallationProgress {
    fn from(other: InstallationProgress) -> Self {
        fidl::InstallationProgress {
            fraction_completed: other.fraction_completed,
            ..Default::default()
        }
    }
}
impl From<fidl::InstallationProgress> for InstallationProgress {
    fn from(progress: fidl::InstallationProgress) -> Self {
        Self { fraction_completed: progress.fraction_completed }
    }
}

/// Wrapper type for [`fidl_fuchsia_update::InstallingData`] which works with
/// [`proptest`].
///
/// Use [`From`] (and [`Into`]) to convert between the fidl type and this one.
#[derive(Clone, Debug, Default, PartialEq, Arbitrary)]
pub struct InstallingData {
    pub update: Option<UpdateInfo>,
    pub installation_progress: Option<InstallationProgress>,
}
impl From<InstallingData> for fidl::InstallingData {
    fn from(other: InstallingData) -> Self {
        fidl::InstallingData {
            update: other.update.map(|ext| ext.into()),
            installation_progress: other.installation_progress.map(|ext| ext.into()),
            ..Default::default()
        }
    }
}
impl From<fidl::InstallingData> for InstallingData {
    fn from(data: fidl::InstallingData) -> Self {
        Self {
            update: data.update.map(|o| o.into()),
            installation_progress: data.installation_progress.map(|o| o.into()),
        }
    }
}

/// Wrapper type for [`fidl_fuchsia_update::InstallationDeferredData`] which
/// works with [`proptest`].
///
/// Use [`From`] (and [`Into`]) to convert between the fidl type and this one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InstallationDeferredData {
    pub update: Option<UpdateInfo>,
    pub deferral_reason: Option<fidl::InstallationDeferralReason>,
}

impl From<InstallationDeferredData> for fidl::InstallationDeferredData {
    fn from(other: InstallationDeferredData) -> Self {
        fidl::InstallationDeferredData {
            update: other.update.map(|ext| ext.into()),
            deferral_reason: other.deferral_reason,
            ..Default::default()
        }
    }
}
impl From<fidl::InstallationDeferredData> for InstallationDeferredData {
    fn from(data: fidl::InstallationDeferredData) -> Self {
        Self { update: data.update.map(|o| o.into()), deferral_reason: data.deferral_reason }
    }
}

// Manually impl Arbitrary because fidl::InstallationDeferralReason does not
// impl Arbitrary. We could have created another wrapper, but we opted not to in
// order to guarantee the ext crate stays in sync with the FIDL for
// InstallationDeferralReason.
impl Arbitrary for InstallationDeferredData {
    type Parameters = ();
    type Strategy = BoxedStrategy<Self>;

    fn arbitrary_with((): Self::Parameters) -> Self::Strategy {
        any::<UpdateInfo>()
            .prop_flat_map(|info| {
                (
                    proptest::option::of(Just(info)),
                    proptest::option::of(Just(
                        fidl::InstallationDeferralReason::CurrentSystemNotCommitted,
                    )),
                )
            })
            .prop_map(|(update, deferral_reason)| Self { update, deferral_reason })
            .boxed()
    }
}

/// Wrapper type for [`fidl_fuchsia_update::UpdateInfo`] which works with
/// [`proptest`].
///
/// Use [`From`] (and [`Into`]) to convert between the fidl type and this one.
#[derive(Clone, Debug, PartialEq, Arbitrary)]
pub struct UpdateInfo {
    #[proptest(strategy = "proptest_util::random_version_available()")]
    pub version_available: Option<String>,
    pub download_size: Option<u64>,
}
impl From<UpdateInfo> for fidl::UpdateInfo {
    fn from(other: UpdateInfo) -> Self {
        fidl::UpdateInfo {
            version_available: other.version_available,
            download_size: other.download_size,
            ..Default::default()
        }
    }
}
impl From<fidl::UpdateInfo> for UpdateInfo {
    fn from(info: fidl::UpdateInfo) -> Self {
        Self { version_available: info.version_available, download_size: info.download_size }
    }
}

/// Wrapper type for [`fidl_fuchsia_update::Initiator`] which works with
/// [`proptest`].
///
/// Use [`From`] (and [`Into`]) to convert between the fidl type and this one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Arbitrary)]
pub enum Initiator {
    User,
    Service,
}

impl From<fidl::Initiator> for Initiator {
    fn from(initiator: fidl::Initiator) -> Self {
        match initiator {
            fidl::Initiator::User => Initiator::User,
            fidl::Initiator::Service => Initiator::Service,
        }
    }
}

impl From<Initiator> for fidl::Initiator {
    fn from(initiator: Initiator) -> Self {
        match initiator {
            Initiator::User => fidl::Initiator::User,
            Initiator::Service => fidl::Initiator::Service,
        }
    }
}

/// An error which can be returned when validating [`CheckOptions`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CheckOptionsDecodeError {
    /// The initiator field was not set.
    #[error("missing field 'initiator'")]
    MissingInitiator,
}

/// Wrapper type for [`fidl_fuchsia_update::CheckOptions`] which validates the
/// options on construction and works with [`proptest`].
///
/// Use [`TryFrom`] (or [`TryInto`]) to convert the fidl type and this one, and
/// [`From`] (or [`Into`]) to convert back to the fidl type.
#[derive(Clone, Debug, PartialEq, Arbitrary, TypedBuilder)]
pub struct CheckOptions {
    pub initiator: Initiator,

    #[builder(default)]
    pub allow_attaching_to_existing_update_check: bool,
}

impl TryFrom<fidl::CheckOptions> for CheckOptions {
    type Error = CheckOptionsDecodeError;

    fn try_from(o: fidl::CheckOptions) -> Result<Self, Self::Error> {
        Ok(Self {
            initiator: o.initiator.ok_or(CheckOptionsDecodeError::MissingInitiator)?.into(),
            allow_attaching_to_existing_update_check: o
                .allow_attaching_to_existing_update_check
                .unwrap_or(false),
        })
    }
}

impl From<CheckOptions> for fidl::CheckOptions {
    fn from(o: CheckOptions) -> Self {
        Self {
            initiator: Some(o.initiator.into()),
            allow_attaching_to_existing_update_check: Some(
                o.allow_attaching_to_existing_update_check,
            ),
            ..Default::default()
        }
    }
}

pub mod proptest_util {
    use proptest::prelude::*;

    prop_compose! {
        /// pick an arbitrary version_available value for `UpdateInfo`
        pub fn random_version_available()(
            version_available in proptest::option::of("[0-9A-Z]{10,20}")
        ) -> Option<String> {
            version_available
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    proptest! {
        // states with the same update info but different progress should merge
        #[test]
        fn test_state_can_merge(
            update_info: Option<UpdateInfo>,
            progress0: Option<InstallationProgress>,
            progress1: Option<InstallationProgress>,
        ) {
            let event0 = State::InstallingUpdate(
                InstallingData {
                    update: update_info.clone(),
                    installation_progress: progress0,
                }
            );
            let event1 = State::InstallingUpdate(
                InstallingData {
                    update: update_info,
                    installation_progress: progress1,
                }
            );
            prop_assert!(event0.can_merge(&event1));
        }

        #[test]
        fn test_attempt_options_can_merge(
            initiator0: Initiator,
            initiator1: Initiator,
        ) {
            let attempt_options0 = AttemptOptions {
                initiator: initiator0,
            };
            let attempt_options1 = AttemptOptions {
                initiator: initiator1,
            };
            prop_assert!(attempt_options0.can_merge(&attempt_options1));
        }

        #[test]
        fn test_state_roundtrips(state: State) {
            let state0: State = state.clone();
            let fidl_intermediate: fidl::State = state.into();
            let state1: State = fidl_intermediate.into();
            prop_assert_eq!(state0, state1);
        }

        #[test]
        fn test_initiator_roundtrips(initiator: Initiator) {
            prop_assert_eq!(
                Initiator::from(fidl::Initiator::from(initiator)),
                initiator
            );
        }

        #[test]
        fn test_check_options_roundtrips(check_options: CheckOptions) {
            prop_assert_eq!(
                CheckOptions::try_from(fidl::CheckOptions::from(check_options.clone())),
                Ok(check_options)
            );
        }

        #[test]
        fn test_check_options_initiator_required(allow_attaching_to_existing_update_check: bool) {
            prop_assert_eq!(
                CheckOptions::try_from(fidl::CheckOptions {
                    initiator: None,
                    allow_attaching_to_existing_update_check: Some(allow_attaching_to_existing_update_check),
                    ..Default::default()
                }),
                Err(CheckOptionsDecodeError::MissingInitiator)
            );
        }
    }

    #[test]
    fn check_options_builder() {
        assert_eq!(
            CheckOptions::builder()
                .initiator(Initiator::User)
                .allow_attaching_to_existing_update_check(true)
                .build(),
            CheckOptions {
                initiator: Initiator::User,
                allow_attaching_to_existing_update_check: true,
            }
        );
    }
}
