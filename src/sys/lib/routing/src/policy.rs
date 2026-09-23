// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use capability_source::{
    AnonymizedAggregateSource, BuiltinSource, CapabilitySource, CapabilityToCapabilitySource,
    ComponentSource, EnvironmentSource, FilteredAggregateProviderSource, FilteredProviderSource,
    FrameworkSource, NamespaceSource, RemotedAtSource, StorageBackingDirectorySource, VoidSource,
};
use cm_config::{
    AllowlistEntry, AllowlistMatcher, CapabilityAllowlistKey, CapabilityAllowlistSource,
    DebugCapabilityKey, SecurityPolicy,
};
use log::{error, warn};
use moniker::{ExtendedMoniker, Moniker};
use std::sync::Arc;
use thiserror::Error;
use zx_status as zx;

use cm_rust::CapabilityTypeName;
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Errors returned by the PolicyChecker and the ScopedPolicyChecker.
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize), serde(rename_all = "snake_case"))]
#[derive(Debug, Clone, Error, PartialEq)]
pub enum PolicyError {
    #[error("security policy disallows \"{policy}\" job policy for \"{moniker}\"")]
    JobPolicyDisallowed { policy: String, moniker: Moniker },

    #[error("security policy disallows \"{policy}\" child policy for \"{moniker}\"")]
    ChildPolicyDisallowed { policy: String, moniker: Moniker },

    #[error(
        "security policy was unable to extract the source from the routed capability at component \"{moniker}\""
    )]
    InvalidCapabilitySource { moniker: ExtendedMoniker },

    #[error(
        "security policy disallows \"{cap}\" from \"{source_moniker}\" being used at \"{target_moniker}\""
    )]
    CapabilityUseDisallowed {
        cap: String,
        source_moniker: ExtendedMoniker,
        target_moniker: Moniker,
    },

    #[error(
        "debug security policy disallows \"{cap}\" from being registered in \
        environment \"{env_name}\" at \"{env_moniker}\""
    )]
    DebugCapabilityUseDisallowed { cap: String, env_moniker: Moniker, env_name: String },
}

impl PolicyError {
    /// Convert this error into its approximate `zx::Status` equivalent.
    pub fn as_zx_status(&self) -> zx::Status {
        zx::Status::ACCESS_DENIED
    }
}

impl From<PolicyError> for ExtendedMoniker {
    fn from(err: PolicyError) -> ExtendedMoniker {
        match err {
            PolicyError::ChildPolicyDisallowed { moniker, .. }
            | PolicyError::DebugCapabilityUseDisallowed { env_moniker: moniker, .. }
            | PolicyError::JobPolicyDisallowed { moniker, .. } => moniker.into(),

            PolicyError::CapabilityUseDisallowed { source_moniker: moniker, .. }
            | PolicyError::InvalidCapabilitySource { moniker } => moniker,
        }
    }
}

/// Evaluates security policy globally across the entire Model and all components.
/// This is used to enforce runtime capability routing restrictions across all
/// components to prevent high privilleged capabilities from being routed to
/// components outside of the list defined in the runtime security policy.
#[derive(Clone, Debug, Default)]
pub struct GlobalPolicyChecker {
    /// The security policy to apply.
    policy: Arc<SecurityPolicy>,
}

impl GlobalPolicyChecker {
    /// Constructs a new PolicyChecker object configured by the SecurityPolicy.
    pub fn new(policy: Arc<SecurityPolicy>) -> Self {
        Self { policy }
    }

    fn get_policy_key(
        capability_source: &CapabilitySource,
    ) -> Result<CapabilityAllowlistKey, PolicyError> {
        Ok(match &capability_source {
            CapabilitySource::Namespace(NamespaceSource { capability, .. }) => {
                CapabilityAllowlistKey {
                    source_moniker: ExtendedMoniker::ComponentManager,
                    source_name: capability
                        .source_name()
                        .ok_or(PolicyError::InvalidCapabilitySource {
                            moniker: capability_source.source_moniker(),
                        })?
                        .clone(),
                    source: CapabilityAllowlistSource::Self_,
                    capability: capability.type_name(),
                }
            }
            CapabilitySource::Component(ComponentSource { capability, moniker })
            | CapabilitySource::StorageBackingDirectory(StorageBackingDirectorySource {
                capability,
                moniker,
                ..
            }) => CapabilityAllowlistKey {
                source_moniker: ExtendedMoniker::ComponentInstance(moniker.clone()),
                source_name: capability
                    .source_name()
                    .ok_or(PolicyError::InvalidCapabilitySource {
                        moniker: capability_source.source_moniker(),
                    })?
                    .clone(),
                source: CapabilityAllowlistSource::Self_,
                capability: capability.type_name(),
            },
            CapabilitySource::Builtin(BuiltinSource { capability, .. }) => CapabilityAllowlistKey {
                source_moniker: ExtendedMoniker::ComponentManager,
                source_name: capability.source_name().clone(),
                source: CapabilityAllowlistSource::Self_,
                capability: capability.type_name(),
            },
            CapabilitySource::Framework(FrameworkSource { capability, moniker }) => {
                CapabilityAllowlistKey {
                    source_moniker: ExtendedMoniker::ComponentInstance(moniker.clone()),
                    source_name: capability.source_name().clone(),
                    source: CapabilityAllowlistSource::Framework,
                    capability: capability.type_name(),
                }
            }
            CapabilitySource::Void(VoidSource { capability, moniker }) => CapabilityAllowlistKey {
                source_moniker: ExtendedMoniker::ComponentInstance(moniker.clone()),
                source_name: capability.source_name().clone(),
                source: CapabilityAllowlistSource::Void,
                capability: capability.type_name(),
            },
            CapabilitySource::Capability(CapabilityToCapabilitySource {
                source_capability,
                moniker,
            }) => CapabilityAllowlistKey {
                source_moniker: ExtendedMoniker::ComponentInstance(moniker.clone()),
                source_name: source_capability
                    .source_name()
                    .ok_or(PolicyError::InvalidCapabilitySource {
                        moniker: capability_source.source_moniker(),
                    })?
                    .clone(),
                source: CapabilityAllowlistSource::Capability,
                capability: source_capability.type_name(),
            },
            CapabilitySource::AnonymizedAggregate(AnonymizedAggregateSource {
                capability,
                moniker,
                ..
            })
            | CapabilitySource::FilteredProvider(FilteredProviderSource {
                capability,
                moniker,
                ..
            })
            | CapabilitySource::FilteredAggregateProvider(FilteredAggregateProviderSource {
                capability,
                moniker,
                ..
            }) => CapabilityAllowlistKey {
                source_moniker: ExtendedMoniker::ComponentInstance(moniker.clone()),
                source_name: capability.source_name().clone(),
                source: CapabilityAllowlistSource::Self_,
                capability: capability.type_name(),
            },
            CapabilitySource::Environment(EnvironmentSource { capability, .. }) => {
                CapabilityAllowlistKey {
                    source_moniker: ExtendedMoniker::ComponentManager,
                    source_name: capability
                        .source_name()
                        .ok_or(PolicyError::InvalidCapabilitySource {
                            moniker: capability_source.source_moniker(),
                        })?
                        .clone(),
                    source: CapabilityAllowlistSource::Environment,
                    capability: capability.type_name(),
                }
            }
            CapabilitySource::RemotedAt(RemotedAtSource { moniker, .. }) => {
                return Err(PolicyError::InvalidCapabilitySource {
                    moniker: ExtendedMoniker::ComponentInstance(moniker.clone()),
                });
            }
        })
    }

    /// Returns Ok(()) if the provided capability source can be routed to the
    /// given target_moniker, else a descriptive PolicyError.
    pub fn can_route_capability<'a>(
        &self,
        capability_source: &'a CapabilitySource,
        target_moniker: &'a Moniker,
    ) -> Result<(), PolicyError> {
        let policy_key = Self::get_policy_key(capability_source).map_err(|e| {
            error!("Security policy could not generate a policy key for `{}`", capability_source);
            e
        })?;

        match self.policy.capability_policy.get(&policy_key) {
            Some(entries) => {
                let parts = target_moniker
                    .path()
                    .iter()
                    .map(|c| AllowlistMatcher::Exact((*c).into()))
                    .collect();
                let entry = AllowlistEntry { matchers: parts };

                // Use the HashSet to find any exact matches quickly.
                if entries.contains(&entry) {
                    return Ok(());
                }

                // Otherwise linear search for any non-exact matches.
                if entries.iter().any(|entry| entry.matches(&target_moniker)) {
                    Ok(())
                } else {
                    warn!(
                        "Security policy prevented `{}` from `{}` being routed to `{}`.",
                        policy_key.source_name, policy_key.source_moniker, target_moniker
                    );
                    Err(PolicyError::CapabilityUseDisallowed {
                        cap: policy_key.source_name.to_string(),
                        source_moniker: policy_key.source_moniker.to_owned(),
                        target_moniker: target_moniker.to_owned(),
                    })
                }
            }
            None => Ok(()),
        }
    }

    /// Returns Ok(()) if the provided debug capability source is allowed to be routed from given
    /// environment.
    pub fn can_register_debug_capability<'a>(
        &self,
        capability_type: CapabilityTypeName,
        name: &'a cm_types::Name,
        env_moniker: &'a Moniker,
        env_name: &'a cm_types::Name,
    ) -> Result<(), PolicyError> {
        let debug_key = DebugCapabilityKey {
            name: name.clone(),
            source: CapabilityAllowlistSource::Self_,
            capability: capability_type,
            env_name: env_name.clone(),
        };
        let route_allowed = match self.policy.debug_capability_policy.get(&debug_key) {
            None => false,
            Some(allowlist_set) => allowlist_set.iter().any(|entry| entry.matches(env_moniker)),
        };
        if route_allowed {
            return Ok(());
        }

        warn!(
            "Debug security policy prevented `{}` from being registered to environment `{}` in `{}`.",
            debug_key.name, env_name, env_moniker,
        );
        Err(PolicyError::DebugCapabilityUseDisallowed {
            cap: debug_key.name.to_string(),
            env_moniker: env_moniker.to_owned(),
            env_name: env_name.to_string(),
        })
    }

    /// Returns Ok(()) if `target_moniker` is allowed to have `on_terminate=REBOOT` set.
    pub fn reboot_on_terminate_allowed(&self, target_moniker: &Moniker) -> Result<(), PolicyError> {
        self.policy
            .child_policy
            .reboot_on_terminate
            .iter()
            .any(|entry| entry.matches(&target_moniker))
            .then_some(())
            .ok_or_else(|| PolicyError::ChildPolicyDisallowed {
                policy: "reboot_on_terminate".to_owned(),
                moniker: target_moniker.to_owned(),
            })
    }
}

/// Evaluates security policy relative to a specific Component (based on that Component's
/// Moniker).
#[derive(Clone)]
pub struct ScopedPolicyChecker {
    /// The security policy to apply.
    policy: Arc<SecurityPolicy>,

    /// The moniker of the component that policy will be evaluated for.
    pub scope: Moniker,
}

impl ScopedPolicyChecker {
    pub fn new(policy: Arc<SecurityPolicy>, scope: Moniker) -> Self {
        ScopedPolicyChecker { policy, scope }
    }

    // This interface is super simple for now since there's only three allowlists. In the future
    // we'll probably want a different interface than an individual function per policy item.

    pub fn ambient_mark_vmo_exec_allowed(&self) -> Result<(), PolicyError> {
        self.policy
            .job_policy
            .ambient_mark_vmo_exec
            .iter()
            .any(|entry| entry.matches(&self.scope))
            .then_some(())
            .ok_or_else(|| PolicyError::JobPolicyDisallowed {
                policy: "ambient_mark_vmo_exec".to_owned(),
                moniker: self.scope.to_owned(),
            })
    }

    pub fn main_process_critical_allowed(&self) -> Result<(), PolicyError> {
        self.policy
            .job_policy
            .main_process_critical
            .iter()
            .any(|entry| entry.matches(&self.scope))
            .then_some(())
            .ok_or_else(|| PolicyError::JobPolicyDisallowed {
                policy: "main_process_critical".to_owned(),
                moniker: self.scope.to_owned(),
            })
    }

    pub fn create_raw_processes_allowed(&self) -> Result<(), PolicyError> {
        self.policy
            .job_policy
            .create_raw_processes
            .iter()
            .any(|entry| entry.matches(&self.scope))
            .then_some(())
            .ok_or_else(|| PolicyError::JobPolicyDisallowed {
                policy: "create_raw_processes".to_owned(),
                moniker: self.scope.to_owned(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use cm_config::{AllowlistEntryBuilder, ChildPolicyAllowlists, JobPolicyAllowlists};
    use moniker::ChildName;
    use std::collections::HashMap;

    #[test]
    fn scoped_policy_checker_vmex() {
        macro_rules! assert_vmex_allowed_matches {
            ($policy:expr, $moniker:expr, $expected:pat) => {
                let result = ScopedPolicyChecker::new($policy.clone(), $moniker.clone())
                    .ambient_mark_vmo_exec_allowed();
                assert_matches!(result, $expected);
            };
        }
        macro_rules! assert_vmex_disallowed {
            ($policy:expr, $moniker:expr) => {
                assert_vmex_allowed_matches!(
                    $policy,
                    $moniker,
                    Err(PolicyError::JobPolicyDisallowed { .. })
                );
            };
        }
        let policy = Arc::new(SecurityPolicy::default());
        assert_vmex_disallowed!(policy, Moniker::root());
        assert_vmex_disallowed!(policy, Moniker::try_from(["foo"]).unwrap());

        let allowed1 = Moniker::try_from(["foo", "bar"]).unwrap();
        let allowed2 = Moniker::try_from(["baz", "fiz"]).unwrap();
        let policy = Arc::new(SecurityPolicy {
            job_policy: JobPolicyAllowlists {
                ambient_mark_vmo_exec: vec![
                    AllowlistEntryBuilder::build_exact_from_moniker(&allowed1),
                    AllowlistEntryBuilder::build_exact_from_moniker(&allowed2),
                ],
                main_process_critical: vec![
                    AllowlistEntryBuilder::build_exact_from_moniker(&allowed1),
                    AllowlistEntryBuilder::build_exact_from_moniker(&allowed2),
                ],
                create_raw_processes: vec![
                    AllowlistEntryBuilder::build_exact_from_moniker(&allowed1),
                    AllowlistEntryBuilder::build_exact_from_moniker(&allowed2),
                ],
            },
            capability_policy: HashMap::new(),
            debug_capability_policy: HashMap::new(),
            child_policy: ChildPolicyAllowlists {
                reboot_on_terminate: vec![
                    AllowlistEntryBuilder::build_exact_from_moniker(&allowed1),
                    AllowlistEntryBuilder::build_exact_from_moniker(&allowed2),
                ],
            },
        });
        assert_vmex_allowed_matches!(policy, allowed1, Ok(()));
        assert_vmex_allowed_matches!(policy, allowed2, Ok(()));
        assert_vmex_disallowed!(policy, Moniker::root());
        assert_vmex_disallowed!(policy, allowed1.parent().unwrap());
        assert_vmex_disallowed!(policy, allowed1.child(ChildName::try_from("baz").unwrap()));
    }

    #[test]
    fn scoped_policy_checker_create_raw_processes() {
        macro_rules! assert_create_raw_processes_allowed_matches {
            ($policy:expr, $moniker:expr, $expected:pat) => {
                let result = ScopedPolicyChecker::new($policy.clone(), $moniker.clone())
                    .create_raw_processes_allowed();
                assert_matches!(result, $expected);
            };
        }
        macro_rules! assert_create_raw_processes_disallowed {
            ($policy:expr, $moniker:expr) => {
                assert_create_raw_processes_allowed_matches!(
                    $policy,
                    $moniker,
                    Err(PolicyError::JobPolicyDisallowed { .. })
                );
            };
        }
        let policy = Arc::new(SecurityPolicy::default());
        assert_create_raw_processes_disallowed!(policy, Moniker::root());
        assert_create_raw_processes_disallowed!(policy, Moniker::try_from(["foo"]).unwrap());

        let allowed1 = Moniker::try_from(["foo", "bar"]).unwrap();
        let allowed2 = Moniker::try_from(["baz", "fiz"]).unwrap();
        let policy = Arc::new(SecurityPolicy {
            job_policy: JobPolicyAllowlists {
                ambient_mark_vmo_exec: vec![],
                main_process_critical: vec![],
                create_raw_processes: vec![
                    AllowlistEntryBuilder::build_exact_from_moniker(&allowed1),
                    AllowlistEntryBuilder::build_exact_from_moniker(&allowed2),
                ],
            },
            capability_policy: HashMap::new(),
            debug_capability_policy: HashMap::new(),
            child_policy: ChildPolicyAllowlists { reboot_on_terminate: vec![] },
        });
        assert_create_raw_processes_allowed_matches!(policy, allowed1, Ok(()));
        assert_create_raw_processes_allowed_matches!(policy, allowed2, Ok(()));
        assert_create_raw_processes_disallowed!(policy, Moniker::root());
        assert_create_raw_processes_disallowed!(policy, allowed1.parent().unwrap());
        assert_create_raw_processes_disallowed!(
            policy,
            allowed1.child(ChildName::try_from("baz").unwrap())
        );
    }

    #[test]
    fn scoped_policy_checker_main_process_critical_allowed() {
        macro_rules! assert_critical_allowed_matches {
            ($policy:expr, $moniker:expr, $expected:pat) => {
                let result = ScopedPolicyChecker::new($policy.clone(), $moniker.clone())
                    .main_process_critical_allowed();
                assert_matches!(result, $expected);
            };
        }
        macro_rules! assert_critical_disallowed {
            ($policy:expr, $moniker:expr) => {
                assert_critical_allowed_matches!(
                    $policy,
                    $moniker,
                    Err(PolicyError::JobPolicyDisallowed { .. })
                );
            };
        }
        let policy = Arc::new(SecurityPolicy::default());
        assert_critical_disallowed!(policy, Moniker::root());
        assert_critical_disallowed!(policy, Moniker::try_from(["foo"]).unwrap());

        let allowed1 = Moniker::try_from(["foo", "bar"]).unwrap();
        let allowed2 = Moniker::try_from(["baz", "fiz"]).unwrap();
        let policy = Arc::new(SecurityPolicy {
            job_policy: JobPolicyAllowlists {
                ambient_mark_vmo_exec: vec![
                    AllowlistEntryBuilder::build_exact_from_moniker(&allowed1),
                    AllowlistEntryBuilder::build_exact_from_moniker(&allowed2),
                ],
                main_process_critical: vec![
                    AllowlistEntryBuilder::build_exact_from_moniker(&allowed1),
                    AllowlistEntryBuilder::build_exact_from_moniker(&allowed2),
                ],
                create_raw_processes: vec![
                    AllowlistEntryBuilder::build_exact_from_moniker(&allowed1),
                    AllowlistEntryBuilder::build_exact_from_moniker(&allowed2),
                ],
            },
            capability_policy: HashMap::new(),
            debug_capability_policy: HashMap::new(),
            child_policy: ChildPolicyAllowlists { reboot_on_terminate: vec![] },
        });
        assert_critical_allowed_matches!(policy, allowed1, Ok(()));
        assert_critical_allowed_matches!(policy, allowed2, Ok(()));
        assert_critical_disallowed!(policy, Moniker::root());
        assert_critical_disallowed!(policy, allowed1.parent().unwrap());
        assert_critical_disallowed!(policy, allowed1.child(ChildName::try_from("baz").unwrap()));
    }

    #[test]
    fn scoped_policy_checker_reboot_policy_allowed() {
        macro_rules! assert_reboot_allowed_matches {
            ($policy:expr, $moniker:expr, $expected:pat) => {
                let result = GlobalPolicyChecker::new($policy.clone())
                    .reboot_on_terminate_allowed(&$moniker);
                assert_matches!(result, $expected);
            };
        }
        macro_rules! assert_reboot_disallowed {
            ($policy:expr, $moniker:expr) => {
                assert_reboot_allowed_matches!(
                    $policy,
                    $moniker,
                    Err(PolicyError::ChildPolicyDisallowed { .. })
                );
            };
        }

        // Empty policy and enabled.
        let policy = Arc::new(SecurityPolicy::default());
        assert_reboot_disallowed!(policy, Moniker::root());
        assert_reboot_disallowed!(policy, Moniker::try_from(["foo"]).unwrap());

        // Nonempty policy.
        let allowed1 = Moniker::try_from(["foo", "bar"]).unwrap();
        let allowed2 = Moniker::try_from(["baz", "fiz"]).unwrap();
        let policy = Arc::new(SecurityPolicy {
            job_policy: JobPolicyAllowlists {
                ambient_mark_vmo_exec: vec![],
                main_process_critical: vec![],
                create_raw_processes: vec![],
            },
            capability_policy: HashMap::new(),
            debug_capability_policy: HashMap::new(),
            child_policy: ChildPolicyAllowlists {
                reboot_on_terminate: vec![
                    AllowlistEntryBuilder::build_exact_from_moniker(&allowed1),
                    AllowlistEntryBuilder::build_exact_from_moniker(&allowed2),
                ],
            },
        });
        assert_reboot_allowed_matches!(policy, allowed1, Ok(()));
        assert_reboot_allowed_matches!(policy, allowed2, Ok(()));
        assert_reboot_disallowed!(policy, Moniker::root());
        assert_reboot_disallowed!(policy, allowed1.parent().unwrap());
        assert_reboot_disallowed!(policy, allowed1.child(ChildName::try_from("baz").unwrap()));
    }
}
