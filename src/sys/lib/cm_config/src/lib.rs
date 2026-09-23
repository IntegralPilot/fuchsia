// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, format_err};
use camino::Utf8PathBuf;
use cm_rust::{CapabilityTypeName, FidlIntoNative};
use cm_types::{Name, ParseError, Url, symmetrical_enums};
use fidl::unpersist;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_component_internal::{
    self as component_internal, BuiltinBootResolver, CapabilityPolicyAllowlists,
    DebugRegistrationPolicyAllowlists, LogDestination, RealmBuilderResolverAndRunner,
};
use log::warn;
use moniker::{ChildName, ExtendedMoniker, Moniker, MonikerError};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use thiserror::Error;
use version_history::{AbiRevision, AbiRevisionError, VersionHistory};

#[cfg(feature = "serde")]
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

/// Runtime configuration options.
/// This configuration intended to be "global", in that the same configuration
/// is applied throughout a given running instance of component_manager.
#[derive(Debug, PartialEq, Eq)]
pub struct RuntimeConfig {
    /// How many children, maximum, are returned by a call to `ChildIterator.next()`.
    pub list_children_batch_size: usize,

    /// Security policy configuration.
    pub security_policy: Arc<SecurityPolicy>,

    /// If true, component manager will be in debug mode. In this mode, component manager
    /// provides the `EventSource` protocol and exposes this protocol. The root component
    /// must be manually started using the LifecycleController protocol in the hub.
    ///
    /// This is done so that an external component (say an integration test) can subscribe
    /// to events before the root component has started.
    pub debug: bool,

    /// Where to look for the trace provider: normal Namespace, or internal RootExposed.
    /// This is ignored if tracing is not enabled as a feature.
    pub trace_provider: TraceProvider,

    /// Enables Component Manager's introspection APIs (RealmQuery, RealmExplorer,
    /// RouteValidator, LifecycleController, etc.) for use by components.
    pub enable_introspection: bool,

    /// If true, component_manager will serve an instance of fuchsia.process.Launcher and use this
    /// launcher for the built-in ELF component runner. The root component can additionally
    /// use and/or offer this service using '/builtin/fuchsia.process.Launcher' from realm.
    // This flag exists because the built-in process launcher *only* works when
    // component_manager runs under a job that has ZX_POL_NEW_PROCESS set to allow, like the root
    // job. Otherwise, the component_manager process cannot directly create process through
    // zx_process_create. When we run component_manager elsewhere, like in test environments, it
    // has to use the fuchsia.process.Launcher service provided through its namespace instead.
    pub use_builtin_process_launcher: bool,

    /// If true, component_manager will maintain a UTC kernel clock and vend write handles through
    /// an instance of `fuchsia.time.Maintenance`. This flag should only be used with the top-level
    /// component_manager.
    pub maintain_utc_clock: bool,

    // The number of threads to use for running component_manager's executor.
    // Value defaults to 1.
    pub num_threads: u8,

    /// The list of capabilities offered from component manager's namespace.
    pub namespace_capabilities: Vec<cm_rust::CapabilityDecl>,

    /// The list of capabilities offered from component manager as built-in capabilities.
    pub builtin_capabilities: Vec<cm_rust::CapabilityDecl>,

    /// URL of the root component to launch. This field is used if no URL
    /// is passed to component manager. If value is passed in both places, then
    /// an error is raised.
    pub root_component_url: Option<Url>,

    /// Path to the component ID index, parsed from
    /// `fuchsia.component.internal.RuntimeConfig.component_id_index_path`.
    pub component_id_index_path: Option<Utf8PathBuf>,

    /// Where to log to.
    pub log_destination: LogDestination,

    /// If true, component manager will log all events dispatched in the topology.
    pub log_all_events: bool,

    /// Which builtin resolver to use for the fuchsia-boot scheme. If not supplied this defaults to
    /// the NONE option.
    pub builtin_boot_resolver: BuiltinBootResolver,

    /// If and how the realm builder resolver and runner are enabled.
    pub realm_builder_resolver_and_runner: RealmBuilderResolverAndRunner,

    /// The enforcement and validation policy to apply to component target ABI revisions.
    pub abi_revision_policy: AbiRevisionPolicy,

    /// Where to get the vmex resource from.
    pub vmex_source: VmexSource,

    /// Components that opt into health checks before an update is committed.
    pub health_check: HealthCheck,

    /// Capabilities to be injected into allowlisted components.
    pub inject_capabilities: Vec<InjectedCapabilities>,

    /// Runtime configuration for Scudo heap allocation, using a 'KEY=VALUE,KEY=VALUE'
    /// format. See Scudo flags documentation for details. It is shadowed by `SCUDO_OPTION`
    //  environ variable from the component manifest.
    pub scudo_options: Option<String>,
}

/// A single security policy allowlist entry.
#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub struct AllowlistEntry {
    // A list of matchers that apply to each child in a moniker.
    // If this list is empty, we must only allow the root moniker.
    pub matchers: Vec<AllowlistMatcher>,
}

impl AllowlistEntry {
    pub fn matches(&self, target_moniker: &Moniker) -> bool {
        let path = target_moniker.path();
        let mut iter = path.iter();

        if self.matchers.is_empty() && !target_moniker.is_root() {
            // If there are no matchers in the allowlist, the moniker must be the root.
            // Anything else will not match.
            return false;
        }

        for matcher in &self.matchers {
            let cur_child = if let Some(target_child) = iter.next() {
                target_child
            } else {
                // We have more matchers, but the moniker has already ended.
                return false;
            };
            match matcher {
                AllowlistMatcher::Exact(child) => {
                    if cur_child != &child {
                        // The child does not exactly match.
                        return false;
                    }
                }
                // Any child is acceptable. Continue with remaining matchers.
                AllowlistMatcher::AnyChild => continue,
                // Any descendant at this point is acceptable.
                AllowlistMatcher::AnyDescendant => return true,
                AllowlistMatcher::AnyDescendantInCollection(expected_collection) => {
                    if let Some(collection) = cur_child.collection() {
                        if collection == expected_collection {
                            // This child is in a collection and the name matches.
                            // Because we allow any descendant, return true immediately.
                            return true;
                        } else {
                            // This child is in a collection but the name does not match.
                            return false;
                        }
                    } else {
                        // This child is not in a collection, so it does not match.
                        return false;
                    }
                }
                AllowlistMatcher::AnyChildInCollection(expected_collection) => {
                    if let Some(collection) = cur_child.collection() {
                        if collection != expected_collection {
                            // This child is in a collection but the name does not match.
                            return false;
                        }
                    } else {
                        // This child is not in a collection, so it does not match.
                        return false;
                    }
                }
            }
        }

        if iter.next().is_some() {
            // We've gone through all the matchers, but there are still children
            // in the moniker. Descendant cases are already handled above, so this
            // must be a failure to match.
            false
        } else {
            true
        }
    }
}

impl FromStr for AllowlistEntry {
    type Err = AllowlistEntryParseError;

    fn from_str(input: &str) -> Result<AllowlistEntry, AllowlistEntryParseError> {
        let entry = if let Some(entry) = input.strip_prefix('/') {
            entry
        } else {
            return Err(AllowlistEntryParseError::NoLeadingSlash(input.to_string()));
        };

        if entry.is_empty() {
            return Ok(AllowlistEntry { matchers: vec![] });
        }

        if entry.contains("**") && !entry.ends_with("**") {
            return Err(AllowlistEntryParseError::DescendantWildcardOnlyAtEnd(input.to_string()));
        }

        let mut parts = vec![];
        for name in entry.split('/') {
            let part = match name {
                "**" => AllowlistMatcher::AnyDescendant,
                "*" => AllowlistMatcher::AnyChild,
                name => {
                    if let Some(collection_name) = name.strip_suffix(":**") {
                        let collection_name = Name::new(collection_name).map_err(|e| {
                            AllowlistEntryParseError::InvalidCollectionName(
                                collection_name.to_string(),
                                e,
                            )
                        })?;
                        AllowlistMatcher::AnyDescendantInCollection(collection_name)
                    } else if let Some(collection_name) = name.strip_suffix(":*") {
                        let collection_name = Name::new(collection_name).map_err(|e| {
                            AllowlistEntryParseError::InvalidCollectionName(
                                collection_name.to_string(),
                                e,
                            )
                        })?;
                        AllowlistMatcher::AnyChildInCollection(collection_name)
                    } else {
                        let child_moniker = ChildName::parse(name).map_err(|e| {
                            AllowlistEntryParseError::InvalidChildName(name.to_string(), e)
                        })?;
                        AllowlistMatcher::Exact(child_moniker)
                    }
                }
            };
            parts.push(part);
        }

        Ok(AllowlistEntry { matchers: parts })
    }
}

impl ToString for AllowlistEntry {
    fn to_string(&self) -> String {
        let mut parts = vec!["".to_string()];
        for matcher in &self.matchers {
            parts.push(match matcher {
                AllowlistMatcher::AnyDescendant => "**".to_string(),
                AllowlistMatcher::AnyChild => "*".to_string(),
                AllowlistMatcher::AnyDescendantInCollection(bounded_name) => {
                    format!("{bounded_name}:**")
                }
                AllowlistMatcher::AnyChildInCollection(bounded_name) => format!("{bounded_name}:*"),
                AllowlistMatcher::Exact(child_name) => child_name.to_string(),
            });
        }
        parts.join("/")
    }
}

#[cfg(feature = "serde")]
impl<'de> Deserialize<'de> for AllowlistEntry {
    fn deserialize<D>(deserializer: D) -> Result<AllowlistEntry, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(de::Error::custom)
    }
}

#[cfg(feature = "serde")]
impl Serialize for AllowlistEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub enum AllowlistMatcher {
    /// Allow the child with this exact ChildName.
    /// Examples: "bar", "foo:bar", "baz"
    Exact(ChildName),
    /// Allow any descendant of this realm.
    /// This is indicated by "**" in a config file.
    AnyDescendant,
    /// Allow any child of this realm.
    /// This is indicated by "*" in a config file.
    AnyChild,
    /// Allow any child of a particular collection in this realm.
    /// This is indicated by "<collection>:*" in a config file.
    AnyChildInCollection(Name),
    /// Allow any descendant of a particular collection in this realm.
    /// This is indicated by "<collection>:**" in a config file.
    AnyDescendantInCollection(Name),
}

pub struct AllowlistEntryBuilder {
    parts: Vec<AllowlistMatcher>,
}

impl AllowlistEntryBuilder {
    pub fn new() -> Self {
        Self { parts: vec![] }
    }

    pub fn build_exact_from_moniker(m: &Moniker) -> AllowlistEntry {
        Self::new().exact_from_moniker(m).build()
    }

    pub fn exact(mut self, name: &str) -> Self {
        self.parts.push(AllowlistMatcher::Exact(ChildName::parse(name).unwrap()));
        self
    }

    pub fn exact_from_moniker(mut self, m: &Moniker) -> Self {
        let path = m.path();
        let parts = path.iter().map(|c| AllowlistMatcher::Exact((*c).into()));
        self.parts.extend(parts);
        self
    }

    pub fn any_child(mut self) -> Self {
        self.parts.push(AllowlistMatcher::AnyChild);
        self
    }

    pub fn any_descendant(mut self) -> AllowlistEntry {
        self.parts.push(AllowlistMatcher::AnyDescendant);
        self.build()
    }

    pub fn any_descendant_in_collection(mut self, collection: &str) -> AllowlistEntry {
        self.parts
            .push(AllowlistMatcher::AnyDescendantInCollection(Name::new(collection).unwrap()));
        self.build()
    }

    pub fn any_child_in_collection(mut self, collection: &str) -> Self {
        self.parts.push(AllowlistMatcher::AnyChildInCollection(Name::new(collection).unwrap()));
        self
    }

    pub fn build(self) -> AllowlistEntry {
        AllowlistEntry { matchers: self.parts }
    }
}

/// Runtime security policy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecurityPolicy {
    /// Allowlists for Zircon job policy.
    pub job_policy: JobPolicyAllowlists,

    /// Capability routing policies. The key contains all the information required
    /// to uniquely identify any routable capability and the set of monikers
    /// define the set of component paths that are allowed to access this specific
    /// capability.
    pub capability_policy: HashMap<CapabilityAllowlistKey, HashSet<AllowlistEntry>>,

    /// Debug Capability routing policies. The key contains all the absolute information
    /// needed to identify a routable capability and the set of DebugCapabilityAllowlistEntries
    /// define the allowed set of routing paths from the capability source to the environment
    /// offering the capability.
    pub debug_capability_policy:
        HashMap<DebugCapabilityKey, HashSet<DebugCapabilityAllowlistEntry>>,

    /// Allowlists component child policy. These allowlists control what components are allowed
    /// to set privileged options on their children.
    pub child_policy: ChildPolicyAllowlists,
}

/// Allowlist key for debug capability allowlists.
/// This defines all portions of the allowlist that do not support globbing.
#[derive(Debug, Hash, PartialEq, Eq, Clone)]
pub struct DebugCapabilityKey {
    pub name: Name,
    pub source: CapabilityAllowlistSource,
    pub capability: CapabilityTypeName,
    pub env_name: Name,
}

/// Represents a single allowed route for a debug capability.
#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub struct DebugCapabilityAllowlistEntry {
    dest: AllowlistEntry,
}

impl DebugCapabilityAllowlistEntry {
    pub fn new(dest: AllowlistEntry) -> Self {
        Self { dest }
    }

    pub fn matches(&self, dest: &Moniker) -> bool {
        self.dest.matches(dest)
    }
}

/// Allowlists for Zircon job policy. Part of runtime security policy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JobPolicyAllowlists {
    /// Entries for components allowed to be given the ZX_POL_AMBIENT_MARK_VMO_EXEC job policy.
    ///
    /// Components must request this policy by including "job_policy_ambient_mark_vmo_exec: true" in
    /// their manifest's program object and must be using the ELF runner.
    /// This is equivalent to the v1 'deprecated-ambient-replace-as-executable' feature.
    pub ambient_mark_vmo_exec: Vec<AllowlistEntry>,

    /// Entries for components allowed to have their original process marked as critical to
    /// component_manager's job.
    ///
    /// Components must request this critical marking by including "main_process_critical: true" in
    /// their manifest's program object and must be using the ELF runner.
    pub main_process_critical: Vec<AllowlistEntry>,

    /// Entries for components allowed to call zx_process_create directly (e.g., do not have
    /// ZX_POL_NEW_PROCESS set to ZX_POL_ACTION_DENY).
    ///
    /// Components must request this policy by including "job_policy_create_raw_processes: true" in
    /// their manifest's program object and must be using the ELF runner.
    pub create_raw_processes: Vec<AllowlistEntry>,
}

/// Allowlists for child option policy. Part of runtime security policy.
#[derive(Debug, Default, PartialEq, Eq, Clone)]
pub struct ChildPolicyAllowlists {
    /// Absolute monikers of component instances allowed to have the
    /// `on_terminate=REBOOT` in their `children` declaration.
    pub reboot_on_terminate: Vec<AllowlistEntry>,
}

/// The available capability sources for capability allow lists. This is a strict
/// subset of all possible Ref types, with equality support.
#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub enum CapabilityAllowlistSource {
    Self_,
    Framework,
    Capability,
    Environment,
    Void,
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum CompatibilityCheckError {
    #[error("Component did not present an ABI revision")]
    AbiRevisionAbsent,
    #[error(transparent)]
    AbiRevisionInvalid(#[from] AbiRevisionError),
}

/// The enforcement and validation policy to apply to component target ABI
/// revisions. By default, enforce ABI compatibility for all components.
#[derive(Debug, PartialEq, Eq, Default, Clone)]
pub struct AbiRevisionPolicy {
    allowlist: Vec<AllowlistEntry>,
}

impl AbiRevisionPolicy {
    pub fn new(allowlist: Vec<AllowlistEntry>) -> Self {
        Self { allowlist }
    }

    /// Check if the abi_revision, if present, is supported by the platform and compatible with the
    /// `AbiRevisionPolicy`. Regardless of the enforcement policy, log a warning if the
    /// ABI revision is missing or not supported by the platform.
    pub fn check_compatibility(
        &self,
        version_history: &VersionHistory,
        moniker: &Moniker,
        abi_revision: Option<AbiRevision>,
    ) -> Result<(), CompatibilityCheckError> {
        let only_warn = self.allowlist.iter().any(|matcher| matcher.matches(moniker));

        let Some(abi_revision) = abi_revision else {
            return if only_warn {
                warn!("Ignoring missing ABI revision in {} because it is allowlisted.", moniker);
                Ok(())
            } else {
                Err(CompatibilityCheckError::AbiRevisionAbsent)
            };
        };

        let abi_error = match version_history.check_abi_revision_for_runtime(abi_revision) {
            Ok(()) => return Ok(()),
            Err(AbiRevisionError::PlatformMismatch { .. })
            | Err(AbiRevisionError::UnstableMismatch { .. })
            | Err(AbiRevisionError::Malformed { .. }) => {
                // TODO(https://fxbug.dev/347724655): Make this an error.
                warn!(
                    "Unsupported platform ABI revision: 0x{}.
This will become an error soon! See https://fxbug.dev/347724655",
                    abi_revision
                );
                return Ok(());
            }
            Err(e @ AbiRevisionError::TooNew { .. })
            | Err(e @ AbiRevisionError::Retired { .. })
            | Err(e @ AbiRevisionError::Invalid) => e,
        };

        if only_warn {
            warn!(
                "Ignoring AbiRevisionError in {} because it is allowlisted: {}",
                moniker, abi_error
            );
            Ok(())
        } else {
            Err(CompatibilityCheckError::AbiRevisionInvalid(abi_error))
        }
    }
}

impl TryFrom<component_internal::AbiRevisionPolicy> for AbiRevisionPolicy {
    type Error = Error;

    fn try_from(abi_revision_policy: component_internal::AbiRevisionPolicy) -> Result<Self, Error> {
        Ok(Self::new(parse_allowlist_entries(&abi_revision_policy.allowlist)?))
    }
}

/// Where to get the Vmex resource from, if this component_manager is hosting bootfs.
/// Defaults to `SystemResource`.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum VmexSource {
    SystemResource,
    Namespace,
}

symmetrical_enums!(VmexSource, component_internal::VmexSource, SystemResource, Namespace);

impl Default for VmexSource {
    fn default() -> Self {
        VmexSource::SystemResource
    }
}

/// Where to look for the trace provider.
/// Defaults to `Namespace`.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum TraceProvider {
    Namespace,
    RootExposed,
}

symmetrical_enums!(TraceProvider, component_internal::TraceProvider, Namespace, RootExposed);

impl Default for TraceProvider {
    fn default() -> Self {
        TraceProvider::Namespace
    }
}

/// Information about the health checks during the update process.
#[derive(Debug, PartialEq, Eq, Default, Clone)]
pub struct HealthCheck {
    pub monikers: Vec<String>,
}

impl HealthCheck {
    pub fn new(monikers: Vec<String>) -> Self {
        Self { monikers }
    }
}

impl TryFrom<component_internal::HealthCheck> for HealthCheck {
    type Error = Error;

    fn try_from(health_check: component_internal::HealthCheck) -> Result<Self, Error> {
        Ok(Self::new(health_check.monikers.unwrap()))
    }
}

/// Allowlist key for capability routing policy. Part of the runtime
/// security policy. This defines all the required keying information to lookup
/// whether a capability exists in the policy map or not.
#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub struct CapabilityAllowlistKey {
    pub source_moniker: ExtendedMoniker,
    pub source_name: Name,
    pub source: CapabilityAllowlistSource,
    pub capability: CapabilityTypeName,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            list_children_batch_size: 1000,
            // security_policy must default to empty to ensure that it fails closed if no
            // configuration is present or it fails to load.
            security_policy: Default::default(),
            debug: false,
            trace_provider: Default::default(),
            enable_introspection: false,
            use_builtin_process_launcher: false,
            maintain_utc_clock: false,
            num_threads: 1,
            namespace_capabilities: vec![],
            builtin_capabilities: vec![],
            root_component_url: Default::default(),
            component_id_index_path: None,
            log_destination: LogDestination::Syslog,
            log_all_events: false,
            builtin_boot_resolver: BuiltinBootResolver::None,
            realm_builder_resolver_and_runner: RealmBuilderResolverAndRunner::None,
            abi_revision_policy: Default::default(),
            vmex_source: Default::default(),
            health_check: Default::default(),
            inject_capabilities: Default::default(),
            scudo_options: Default::default(),
        }
    }
}

impl RuntimeConfig {
    pub fn new_from_bytes(bytes: &Vec<u8>) -> Result<Self, Error> {
        Ok(Self::try_from(unpersist::<component_internal::Config>(&bytes)?)?)
    }

    fn translate_namespace_capabilities(
        capabilities: Option<Vec<fdecl::Capability>>,
    ) -> Result<Vec<cm_rust::CapabilityDecl>, Error> {
        let capabilities = capabilities.unwrap_or(vec![]);
        if let Some(c) = capabilities.iter().find(|c| {
            !matches!(c, fdecl::Capability::Protocol(_) | fdecl::Capability::Directory(_))
        }) {
            return Err(format_err!("Type unsupported for namespace capability: {:?}", c));
        }
        cm_fidl_validator::validate_namespace_capabilities(&capabilities)?;
        Ok(capabilities.into_iter().map(FidlIntoNative::fidl_into_native).collect())
    }

    fn translate_builtin_capabilities(
        capabilities: Option<Vec<fdecl::Capability>>,
    ) -> Result<Vec<cm_rust::CapabilityDecl>, Error> {
        let capabilities = capabilities.unwrap_or(vec![]);
        cm_fidl_validator::validate_builtin_capabilities(&capabilities)?;
        Ok(capabilities.into_iter().map(FidlIntoNative::fidl_into_native).collect())
    }
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum AllowlistEntryParseError {
    #[error("Invalid child moniker ({0:?}) in allowlist entry: {1:?}")]
    InvalidChildName(String, #[source] MonikerError),
    #[error("Invalid collection name ({0:?}) in allowlist entry: {1:?}")]
    InvalidCollectionName(String, #[source] ParseError),
    #[error("Allowlist entry ({0:?}) must start with a '/'")]
    NoLeadingSlash(String),
    #[error("Allowlist entry ({0:?}) must have '**' wildcard only at the end")]
    DescendantWildcardOnlyAtEnd(String),
}

fn parse_allowlist_entries(strs: &Option<Vec<String>>) -> Result<Vec<AllowlistEntry>, Error> {
    let strs = match strs {
        Some(strs) => strs,
        None => return Ok(Vec::new()),
    };

    let mut entries = vec![];
    for input in strs {
        entries.push(input.parse()?);
    }
    Ok(entries)
}

fn parse_optional_vec<T, S: TryInto<T>>(src: Option<Vec<S>>) -> Result<Vec<T>, Error>
where
    Result<Vec<T>, Error>: FromIterator<Result<T, <S as TryInto<T>>::Error>>,
{
    src.unwrap_or_default().into_iter().map(TryInto::try_into).collect()
}

fn as_usize_or_default(value: Option<u32>, default: usize) -> usize {
    match value {
        Some(value) => value as usize,
        None => default,
    }
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum PolicyConfigError {
    #[error("Capability source name was empty in a capability policy entry.")]
    EmptyCapabilitySourceName,
    #[error("Capability type was empty in a capability policy entry.")]
    EmptyAllowlistedCapability,
    #[error("Debug registration type was empty in a debug policy entry.")]
    EmptyAllowlistedDebugRegistration,
    #[error("Target moniker was empty in a debug policy entry.")]
    EmptyTargetMonikerDebugRegistration,
    #[error("Environment name was empty or invalid in a debug policy entry.")]
    InvalidEnvironmentNameDebugRegistration,
    #[error("Capability from type was empty in a capability policy entry.")]
    EmptyFromType,
    #[error("Capability source_moniker was empty in a capability policy entry.")]
    EmptySourceMoniker,
    #[error("Invalid source capability.")]
    InvalidSourceCapability,
    #[error("Unsupported allowlist capability type")]
    UnsupportedAllowlistedCapability,
}

impl TryFrom<component_internal::Config> for RuntimeConfig {
    type Error = Error;

    fn try_from(config: component_internal::Config) -> Result<Self, Error> {
        let default = RuntimeConfig::default();

        let list_children_batch_size =
            as_usize_or_default(config.list_children_batch_size, default.list_children_batch_size);
        let num_threads = config.num_threads.unwrap_or(default.num_threads);

        let root_component_url = config.root_component_url.map(Url::new).transpose()?;

        let security_policy = config
            .security_policy
            .map(SecurityPolicy::try_from)
            .transpose()
            .context("Unable to parse security policy")?
            .unwrap_or_default();

        let abi_revision_policy = config
            .abi_revision_policy
            .map(AbiRevisionPolicy::try_from)
            .transpose()
            .context("Unable to parse ABI revision policy")?
            .unwrap_or_default();

        let vmex_source = config.vmex_source.map(VmexSource::from).unwrap_or_default();

        let trace_provider = config.trace_provider.map(TraceProvider::from).unwrap_or_default();

        let health_check = config
            .health_check
            .map(HealthCheck::try_from)
            .transpose()
            .context("Unable to parse health checks policy")?
            .unwrap_or_default();

        Ok(RuntimeConfig {
            list_children_batch_size,
            security_policy: Arc::new(security_policy),
            namespace_capabilities: Self::translate_namespace_capabilities(
                config.namespace_capabilities,
            )?,
            builtin_capabilities: Self::translate_builtin_capabilities(
                config.builtin_capabilities,
            )?,
            debug: config.debug.unwrap_or(default.debug),
            trace_provider,
            enable_introspection: config
                .enable_introspection
                .unwrap_or(default.enable_introspection),
            use_builtin_process_launcher: config
                .use_builtin_process_launcher
                .unwrap_or(default.use_builtin_process_launcher),
            maintain_utc_clock: config.maintain_utc_clock.unwrap_or(default.maintain_utc_clock),
            num_threads,
            root_component_url,
            component_id_index_path: config.component_id_index_path.map(Into::into),
            log_destination: config.log_destination.unwrap_or(default.log_destination),
            log_all_events: config.log_all_events.unwrap_or(default.log_all_events),
            builtin_boot_resolver: config
                .builtin_boot_resolver
                .unwrap_or(default.builtin_boot_resolver),
            realm_builder_resolver_and_runner: config
                .realm_builder_resolver_and_runner
                .unwrap_or(default.realm_builder_resolver_and_runner),
            abi_revision_policy,
            vmex_source,
            health_check,
            inject_capabilities: parse_optional_vec(config.inject_capabilities)
                .context("Unable to parse injected capabilities")?,
            scudo_options: config.scudo_options,
        })
    }
}

fn parse_capability_policy(
    capability_policy: Option<CapabilityPolicyAllowlists>,
) -> Result<HashMap<CapabilityAllowlistKey, HashSet<AllowlistEntry>>, Error> {
    let capability_policy = if let Some(capability_policy) = capability_policy {
        if let Some(allowlist) = capability_policy.allowlist {
            let mut policies = HashMap::new();
            for e in allowlist.into_iter() {
                let source_moniker = ExtendedMoniker::parse_str(
                    e.source_moniker
                        .as_deref()
                        .ok_or_else(|| Error::new(PolicyConfigError::EmptySourceMoniker))?,
                )?;
                let source_name = if let Some(source_name) = e.source_name {
                    Ok(source_name
                        .parse()
                        .map_err(|_| Error::new(PolicyConfigError::InvalidSourceCapability))?)
                } else {
                    Err(PolicyConfigError::EmptyCapabilitySourceName)
                }?;
                let source = match e.source {
                    Some(fdecl::Ref::Self_(_)) => Ok(CapabilityAllowlistSource::Self_),
                    Some(fdecl::Ref::Framework(_)) => Ok(CapabilityAllowlistSource::Framework),
                    Some(fdecl::Ref::Capability(_)) => Ok(CapabilityAllowlistSource::Capability),
                    Some(fdecl::Ref::Environment(_)) => Ok(CapabilityAllowlistSource::Environment),
                    _ => Err(Error::new(PolicyConfigError::InvalidSourceCapability)),
                }?;

                let capability = if let Some(capability) = e.capability.as_ref() {
                    match &capability {
                        component_internal::AllowlistedCapability::Directory(_) => {
                            Ok(CapabilityTypeName::Directory)
                        }
                        component_internal::AllowlistedCapability::Protocol(_) => {
                            Ok(CapabilityTypeName::Protocol)
                        }
                        component_internal::AllowlistedCapability::Service(_) => {
                            Ok(CapabilityTypeName::Service)
                        }
                        component_internal::AllowlistedCapability::Storage(_) => {
                            Ok(CapabilityTypeName::Storage)
                        }
                        component_internal::AllowlistedCapability::Runner(_) => {
                            Ok(CapabilityTypeName::Runner)
                        }
                        component_internal::AllowlistedCapability::Resolver(_) => {
                            Ok(CapabilityTypeName::Resolver)
                        }
                        _ => Err(Error::new(PolicyConfigError::EmptyAllowlistedCapability)),
                    }
                } else {
                    Err(Error::new(PolicyConfigError::EmptyAllowlistedCapability))
                }?;

                let target_monikers =
                    HashSet::from_iter(parse_allowlist_entries(&e.target_monikers)?);

                policies.insert(
                    CapabilityAllowlistKey { source_moniker, source_name, source, capability },
                    target_monikers,
                );
            }
            policies
        } else {
            HashMap::new()
        }
    } else {
        HashMap::new()
    };
    Ok(capability_policy)
}

fn parse_debug_capability_policy(
    debug_registration_policy: Option<DebugRegistrationPolicyAllowlists>,
) -> Result<HashMap<DebugCapabilityKey, HashSet<DebugCapabilityAllowlistEntry>>, Error> {
    let debug_capability_policy = if let Some(debug_capability_policy) = debug_registration_policy {
        if let Some(allowlist) = debug_capability_policy.allowlist {
            let mut policies: HashMap<DebugCapabilityKey, HashSet<DebugCapabilityAllowlistEntry>> =
                HashMap::new();
            for e in allowlist.into_iter() {
                let moniker = e
                    .moniker
                    .as_deref()
                    .ok_or_else(|| Error::new(PolicyConfigError::EmptySourceMoniker))?
                    .parse()?;
                let name = if let Some(name) = e.name.as_ref() {
                    Ok(name
                        .parse()
                        .map_err(|_| Error::new(PolicyConfigError::InvalidSourceCapability))?)
                } else {
                    Err(PolicyConfigError::EmptyCapabilitySourceName)
                }?;

                let capability = if let Some(capability) = e.debug.as_ref() {
                    match &capability {
                        component_internal::AllowlistedDebugRegistration::Protocol(_) => {
                            Ok(CapabilityTypeName::Protocol)
                        }
                        _ => Err(Error::new(PolicyConfigError::EmptyAllowlistedDebugRegistration)),
                    }
                } else {
                    Err(Error::new(PolicyConfigError::EmptyAllowlistedDebugRegistration))
                }?;

                let env_name = e
                    .environment_name
                    .map(|n| n.parse().ok())
                    .flatten()
                    .ok_or(PolicyConfigError::InvalidEnvironmentNameDebugRegistration)?;

                let key = DebugCapabilityKey {
                    name,
                    source: CapabilityAllowlistSource::Self_,
                    capability,
                    env_name,
                };
                let value = DebugCapabilityAllowlistEntry::new(moniker);
                if let Some(h) = policies.get_mut(&key) {
                    h.insert(value);
                } else {
                    policies.insert(key, vec![value].into_iter().collect());
                }
            }
            policies
        } else {
            HashMap::new()
        }
    } else {
        HashMap::new()
    };
    Ok(debug_capability_policy)
}

impl TryFrom<component_internal::SecurityPolicy> for SecurityPolicy {
    type Error = Error;

    fn try_from(security_policy: component_internal::SecurityPolicy) -> Result<Self, Error> {
        let job_policy = if let Some(job_policy) = &security_policy.job_policy {
            let ambient_mark_vmo_exec = parse_allowlist_entries(&job_policy.ambient_mark_vmo_exec)?;
            let main_process_critical = parse_allowlist_entries(&job_policy.main_process_critical)?;
            let create_raw_processes = parse_allowlist_entries(&job_policy.create_raw_processes)?;
            JobPolicyAllowlists {
                ambient_mark_vmo_exec,
                main_process_critical,
                create_raw_processes,
            }
        } else {
            JobPolicyAllowlists::default()
        };

        let capability_policy = parse_capability_policy(security_policy.capability_policy)?;

        let debug_capability_policy =
            parse_debug_capability_policy(security_policy.debug_registration_policy)?;

        let child_policy = if let Some(child_policy) = &security_policy.child_policy {
            let reboot_on_terminate = parse_allowlist_entries(&child_policy.reboot_on_terminate)?;
            ChildPolicyAllowlists { reboot_on_terminate }
        } else {
            ChildPolicyAllowlists::default()
        };

        Ok(SecurityPolicy { job_policy, capability_policy, debug_capability_policy, child_policy })
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
#[cfg_attr(
    feature = "serde",
    derive(Deserialize, Serialize),
    serde(deny_unknown_fields, rename_all = "snake_case", tag = "type")
)]
pub enum InjectedUse {
    Protocol(InjectedUseProtocol),
}

impl TryFrom<component_internal::InjectedUse> for InjectedUse {
    type Error = Error;

    fn try_from(value: component_internal::InjectedUse) -> Result<Self, Error> {
        match value {
            component_internal::InjectedUse::Protocol(protocol) => {
                Ok(InjectedUse::Protocol(protocol.try_into()?))
            }
            other => Err(format_err!("Invalid InjectedUse value: {other:?}")),
        }
    }
}

impl Into<component_internal::InjectedUse> for InjectedUse {
    fn into(self) -> component_internal::InjectedUse {
        match self {
            InjectedUse::Protocol(protocol) => {
                component_internal::InjectedUse::Protocol(protocol.into())
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize), serde(deny_unknown_fields))]
pub struct InjectedUseProtocol {
    pub source_name: cm_types::Name,
    pub target_path: cm_types::Path,
}

impl TryFrom<component_internal::InjectedUseProtocol> for InjectedUseProtocol {
    type Error = Error;

    fn try_from(value: component_internal::InjectedUseProtocol) -> Result<Self, Error> {
        Ok(InjectedUseProtocol {
            source_name: value.source_name.context("Missing source_name")?.parse()?,
            target_path: value.target_path.context("Missing target_path")?.parse()?,
        })
    }
}

impl Into<component_internal::InjectedUseProtocol> for InjectedUseProtocol {
    fn into(self) -> component_internal::InjectedUseProtocol {
        component_internal::InjectedUseProtocol {
            source_name: Some(self.source_name.to_string()),
            target_path: Some(self.target_path.to_string()),
            ..component_internal::InjectedUseProtocol::default()
        }
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize), serde(deny_unknown_fields))]
pub struct InjectedCapabilities {
    /// Components that will have these capabilities injected into.
    pub components: Vec<AllowlistEntry>,

    /// Capabilities to be injected.
    #[cfg_attr(feature = "serde", serde(rename = "use"))]
    pub use_: Vec<InjectedUse>,
}

impl InjectedCapabilities {
    pub fn new(components: Vec<AllowlistEntry>, use_: Vec<InjectedUse>) -> Self {
        Self { components, use_ }
    }
}

impl TryFrom<component_internal::InjectedCapabilities> for InjectedCapabilities {
    type Error = Error;

    fn try_from(value: component_internal::InjectedCapabilities) -> Result<Self, Error> {
        Ok(Self::new(parse_allowlist_entries(&value.components)?, parse_optional_vec(value.use_)?))
    }
}

impl Into<component_internal::InjectedCapabilities> for InjectedCapabilities {
    fn into(self) -> component_internal::InjectedCapabilities {
        component_internal::InjectedCapabilities {
            components: Some(self.components.into_iter().map(|entry| entry.to_string()).collect()),
            use_: Some(self.use_.into_iter().map(Into::into).collect()),
            ..component_internal::InjectedCapabilities::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fidl_fuchsia_io as fio;
    use version_history::{ApiLevel, Version, VersionVec};

    const FOO_PKG_URL: &str = "fuchsia-pkg://fuchsia.com/foo#meta/foo.cm";

    macro_rules! test_function_ok {
        ( $function:path, $($test_name:ident => ($input:expr, $expected:expr)),+ ) => {
            $(
                #[test]
                fn $test_name() {
                    assert_matches!($function($input), Ok(v) if v == $expected);
                }
            )+
        };
    }

    macro_rules! test_function_err {
        ( $function:path, $($test_name:ident => ($input:expr, $type:ty, $expected:expr)),+ ) => {
            $(
                #[test]
                fn $test_name() {
                    assert_eq!(*$function($input).unwrap_err().downcast_ref::<$type>().unwrap(), $expected);
                }
            )+
        };
    }

    macro_rules! test_config_ok {
        ( $($test_name:ident => ($input:expr, $expected:expr)),+ $(,)? ) => {
            test_function_ok! { RuntimeConfig::try_from, $($test_name => ($input, $expected)),+ }
        };
    }

    macro_rules! test_config_err {
        ( $($test_name:ident => ($input:expr, $type:ty, $expected:expr)),+ $(,)? ) => {
            test_function_err! { RuntimeConfig::try_from, $($test_name => ($input, $type, $expected)),+ }
        };
    }

    test_config_ok! {
        all_fields_none => (component_internal::Config {
            debug: None,
            trace_provider: None,
            enable_introspection: None,
            list_children_batch_size: None,
            security_policy: None,
            maintain_utc_clock: None,
            use_builtin_process_launcher: None,
            num_threads: None,
            namespace_capabilities: None,
            builtin_capabilities: None,
            root_component_url: None,
            component_id_index_path: None,
            ..Default::default()
        }, RuntimeConfig::default()),
        all_leaf_nodes_none => (component_internal::Config {
            debug: Some(false),
            trace_provider: Some(component_internal::TraceProvider::Namespace),
            enable_introspection: Some(false),
            list_children_batch_size: Some(5),
            maintain_utc_clock: Some(false),
            use_builtin_process_launcher: Some(true),
            security_policy: Some(component_internal::SecurityPolicy {
                job_policy: Some(component_internal::JobPolicyAllowlists {
                    main_process_critical: None,
                    ambient_mark_vmo_exec: None,
                    create_raw_processes: None,
                    ..Default::default()
                }),
                capability_policy: None,
                ..Default::default()
            }),
            num_threads: Some(10),
            namespace_capabilities: None,
            builtin_capabilities: None,
            root_component_url: None,
            component_id_index_path: None,
            log_destination: None,
            log_all_events: None,
            ..Default::default()
        }, RuntimeConfig {
            debug: false,
            trace_provider: TraceProvider::Namespace,
            enable_introspection: false,
            list_children_batch_size: 5,
            maintain_utc_clock: false,
            use_builtin_process_launcher:true,
            num_threads: 10,
            ..Default::default() }),
        all_fields_some => (
            component_internal::Config {
                debug: Some(true),
                trace_provider: Some(component_internal::TraceProvider::RootExposed),
                enable_introspection: Some(true),
                list_children_batch_size: Some(42),
                maintain_utc_clock: Some(true),
                use_builtin_process_launcher: Some(false),
                security_policy: Some(component_internal::SecurityPolicy {
                    job_policy: Some(component_internal::JobPolicyAllowlists {
                        main_process_critical: Some(vec!["/something/important".to_string()]),
                        ambient_mark_vmo_exec: Some(vec!["/".to_string(), "/foo/bar".to_string()]),
                        create_raw_processes: Some(vec!["/another/thing".to_string()]),
                        ..Default::default()
                    }),
                    capability_policy: Some(component_internal::CapabilityPolicyAllowlists {
                        allowlist: Some(vec![
                        component_internal::CapabilityAllowlistEntry {
                            source_moniker: Some("<component_manager>".to_string()),
                            source_name: Some("fuchsia.kernel.MmioResource".to_string()),
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                            capability: Some(component_internal::AllowlistedCapability::Protocol(component_internal::AllowlistedProtocol::default())),
                            target_monikers: Some(vec![
                                "/bootstrap".to_string(),
                                "/core/**".to_string(),
                                "/core/test_manager/tests:**".to_string()
                            ]),
                            ..Default::default()
                        },
                    ]), ..Default::default()}),
                    debug_registration_policy: Some(component_internal::DebugRegistrationPolicyAllowlists{
                        allowlist: Some(vec![
                            component_internal::DebugRegistrationAllowlistEntry {
                                name: Some("fuchsia.foo.bar".to_string()),
                                debug: Some(component_internal::AllowlistedDebugRegistration::Protocol(component_internal::AllowlistedProtocol::default())),
                                moniker: Some("/foo/bar".to_string()),
                                environment_name: Some("bar_env1".to_string()),
                                ..Default::default()
                            },
                            component_internal::DebugRegistrationAllowlistEntry {
                                name: Some("fuchsia.foo.bar".to_string()),
                                debug: Some(component_internal::AllowlistedDebugRegistration::Protocol(component_internal::AllowlistedProtocol::default())),
                                moniker: Some("/foo".to_string()),
                                environment_name: Some("foo_env1".to_string()),
                                ..Default::default()
                            },
                            component_internal::DebugRegistrationAllowlistEntry {
                                name: Some("fuchsia.foo.baz".to_string()),
                                debug: Some(component_internal::AllowlistedDebugRegistration::Protocol(component_internal::AllowlistedProtocol::default())),
                                moniker: Some("/foo/**".to_string()),
                                environment_name: Some("foo_env2".to_string()),
                                ..Default::default()
                            },
                            component_internal::DebugRegistrationAllowlistEntry {
                                name: Some("fuchsia.foo.baz".to_string()),
                                debug: Some(component_internal::AllowlistedDebugRegistration::Protocol(component_internal::AllowlistedProtocol::default())),
                                moniker: Some("/root".to_string()),
                                environment_name: Some("root_env".to_string()),
                                ..Default::default()
                            },
                        ]), ..Default::default()}),
                    child_policy: Some(component_internal::ChildPolicyAllowlists {
                        reboot_on_terminate: Some(vec!["/something/important".to_string()]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                num_threads: Some(24),
                namespace_capabilities: Some(vec![
                    fdecl::Capability::Protocol(fdecl::Protocol {
                        name: Some("foo_svc".into()),
                        source_path: Some("/svc/foo".into()),
                        ..Default::default()
                    }),
                    fdecl::Capability::Directory(fdecl::Directory {
                        name: Some("bar_dir".into()),
                        source_path: Some("/bar".into()),
                        rights: Some(fio::Operations::CONNECT),
                        ..Default::default()
                    }),
                ]),
                builtin_capabilities: Some(vec![
                    fdecl::Capability::Protocol(fdecl::Protocol {
                        name: Some("foo_protocol".into()),
                        source_path: None,
                        ..Default::default()
                    }),
                ]),
                root_component_url: Some(FOO_PKG_URL.to_string()),
                component_id_index_path: Some("/boot/config/component_id_index".to_string()),
                log_destination: Some(component_internal::LogDestination::Klog),
                log_all_events: Some(true),
                builtin_boot_resolver: Some(component_internal::BuiltinBootResolver::None),
                realm_builder_resolver_and_runner: Some(component_internal::RealmBuilderResolverAndRunner::None),
                abi_revision_policy: Some(component_internal::AbiRevisionPolicy{
                    allowlist: Some(vec!["/baz".to_string(), "/qux/**".to_string()]),
                    ..Default::default()
                }),
                vmex_source: Some(component_internal::VmexSource::Namespace),
                health_check: Some(component_internal::HealthCheck{ monikers: Some(vec!()), ..Default::default()}),
                ..Default::default()
            },
            RuntimeConfig {
                abi_revision_policy: AbiRevisionPolicy::new(vec![
                    AllowlistEntryBuilder::new().exact("baz").build(),
                    AllowlistEntryBuilder::new().exact("qux").any_descendant(),
                ]),
                debug: true,
                trace_provider: TraceProvider::RootExposed,
                enable_introspection: true,
                list_children_batch_size: 42,
                maintain_utc_clock: true,
                use_builtin_process_launcher: false,
                security_policy: Arc::new(SecurityPolicy {
                    job_policy: JobPolicyAllowlists {
                        ambient_mark_vmo_exec: vec![
                            AllowlistEntryBuilder::new().build(),
                            AllowlistEntryBuilder::new().exact("foo").exact("bar").build(),
                        ],
                        main_process_critical: vec![
                            AllowlistEntryBuilder::new().exact("something").exact("important").build(),
                        ],
                        create_raw_processes: vec![
                            AllowlistEntryBuilder::new().exact("another").exact("thing").build(),
                        ],
                    },
                    capability_policy: HashMap::from_iter(vec![
                        (CapabilityAllowlistKey {
                            source_moniker: ExtendedMoniker::ComponentManager,
                            source_name: "fuchsia.kernel.MmioResource".parse().unwrap(),
                            source: CapabilityAllowlistSource::Self_,
                            capability: CapabilityTypeName::Protocol,
                        },
                        HashSet::from_iter(vec![
                            AllowlistEntryBuilder::new().exact("bootstrap").build(),
                            AllowlistEntryBuilder::new().exact("core").any_descendant(),
                            AllowlistEntryBuilder::new().exact("core").exact("test_manager").any_descendant_in_collection("tests"),
                        ].iter().cloned())
                        ),
                    ].iter().cloned()),
                    debug_capability_policy: HashMap::from_iter(vec![
                        (
                            DebugCapabilityKey {
                                name: "fuchsia.foo.bar".parse().unwrap(),
                                source: CapabilityAllowlistSource::Self_,
                                capability: CapabilityTypeName::Protocol,
                                env_name: "bar_env1".parse().unwrap(),
                            },
                            HashSet::from_iter(vec![
                                DebugCapabilityAllowlistEntry::new(
                                    AllowlistEntryBuilder::new().exact("foo").exact("bar").build(),
                                )
                            ])
                        ),
                        (
                            DebugCapabilityKey {
                                name: "fuchsia.foo.bar".parse().unwrap(),
                                source: CapabilityAllowlistSource::Self_,
                                capability: CapabilityTypeName::Protocol,
                                env_name: "foo_env1".parse().unwrap(),
                            },
                            HashSet::from_iter(vec![
                                DebugCapabilityAllowlistEntry::new(
                                    AllowlistEntryBuilder::new().exact("foo").build(),
                                )
                            ])
                        ),
                        (
                            DebugCapabilityKey {
                                name: "fuchsia.foo.baz".parse().unwrap(),
                                source: CapabilityAllowlistSource::Self_,
                                capability: CapabilityTypeName::Protocol,
                                env_name: "foo_env2".parse().unwrap(),
                            },
                            HashSet::from_iter(vec![
                                DebugCapabilityAllowlistEntry::new(
                                    AllowlistEntryBuilder::new().exact("foo").any_descendant(),
                                )
                            ])
                        ),
                        (
                            DebugCapabilityKey {
                                name: "fuchsia.foo.baz".parse().unwrap(),
                                source: CapabilityAllowlistSource::Self_,
                                capability: CapabilityTypeName::Protocol,
                                env_name: "root_env".parse().unwrap(),
                            },
                            HashSet::from_iter(vec![
                                DebugCapabilityAllowlistEntry::new(
                                    AllowlistEntryBuilder::new().exact("root").build(),
                                )
                            ])
                        ),
                    ]),
                    child_policy: ChildPolicyAllowlists {
                        reboot_on_terminate: vec![
                            AllowlistEntryBuilder::new().exact("something").exact("important").build(),
                        ],
                    },
                }),
                num_threads: 24,
                namespace_capabilities: vec![
                    cm_rust::CapabilityDecl::Protocol(cm_rust::ProtocolDecl {
                        name: "foo_svc".parse().unwrap(),
                        source_path: Some("/svc/foo".parse().unwrap()),
                        delivery: Default::default(),
                    }),
                    cm_rust::CapabilityDecl::Directory(cm_rust::DirectoryDecl {
                        name: "bar_dir".parse().unwrap(),
                        source_path: Some("/bar".parse().unwrap()),
                        rights: fio::Operations::CONNECT,
                    }),
                ],
                builtin_capabilities: vec![
                    cm_rust::CapabilityDecl::Protocol(cm_rust::ProtocolDecl {
                        name: "foo_protocol".parse().unwrap(),
                        source_path: None,
                        delivery: Default::default(),
                    }),
                ],
                root_component_url: Some(Url::new(FOO_PKG_URL.to_string()).unwrap()),
                component_id_index_path: Some("/boot/config/component_id_index".into()),
                log_destination: LogDestination::Klog,
                log_all_events: true,
                builtin_boot_resolver: BuiltinBootResolver::None,
                realm_builder_resolver_and_runner: RealmBuilderResolverAndRunner::None,
                vmex_source: VmexSource::Namespace,
                health_check: HealthCheck{monikers: vec!()},
                inject_capabilities: vec![],
                scudo_options: None,
            }
        ),
    }

    test_config_err! {
        invalid_job_policy => (
            component_internal::Config {
                debug: None,
                trace_provider: None,
                enable_introspection: None,
                list_children_batch_size: None,
                maintain_utc_clock: None,
                use_builtin_process_launcher: None,
                security_policy: Some(component_internal::SecurityPolicy {
                    job_policy: Some(component_internal::JobPolicyAllowlists {
                        main_process_critical: None,
                        ambient_mark_vmo_exec: Some(vec!["/".to_string(), "bad".to_string()]),
                        create_raw_processes: None,
                        ..Default::default()
                    }),
                    capability_policy: None,
                    ..Default::default()
                }),
                num_threads: None,
                namespace_capabilities: None,
                builtin_capabilities: None,
                root_component_url: None,
                component_id_index_path: None,
                ..Default::default()
            },
            AllowlistEntryParseError,
            AllowlistEntryParseError::NoLeadingSlash(
                "bad".into(),
            )
        ),
        invalid_capability_policy_empty_allowlist_cap => (
            component_internal::Config {
                debug: None,
                trace_provider: None,
                enable_introspection: None,
                list_children_batch_size: None,
                maintain_utc_clock: None,
                use_builtin_process_launcher: None,
                security_policy: Some(component_internal::SecurityPolicy {
                    job_policy: None,
                    capability_policy: Some(component_internal::CapabilityPolicyAllowlists {
                        allowlist: Some(vec![
                        component_internal::CapabilityAllowlistEntry {
                            source_moniker: Some("<component_manager>".to_string()),
                            source_name: Some("fuchsia.kernel.MmioResource".to_string()),
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                            capability: None,
                            target_monikers: Some(vec!["/core".to_string()]),
                            ..Default::default()
                        }]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                num_threads: None,
                namespace_capabilities: None,
                builtin_capabilities: None,
                root_component_url: None,
                component_id_index_path: None,
                ..Default::default()
            },
            PolicyConfigError,
            PolicyConfigError::EmptyAllowlistedCapability
        ),
        invalid_capability_policy_empty_source_moniker => (
            component_internal::Config {
                debug: None,
                trace_provider: None,
                enable_introspection: None,
                list_children_batch_size: None,
                maintain_utc_clock: None,
                use_builtin_process_launcher: None,
                security_policy: Some(component_internal::SecurityPolicy {
                    job_policy: None,
                    capability_policy: Some(component_internal::CapabilityPolicyAllowlists {
                        allowlist: Some(vec![
                        component_internal::CapabilityAllowlistEntry {
                            source_moniker: None,
                            source_name: Some("fuchsia.kernel.MmioResource".to_string()),
                            capability: Some(component_internal::AllowlistedCapability::Protocol(component_internal::AllowlistedProtocol::default())),
                            target_monikers: Some(vec!["/core".to_string()]),
                            ..Default::default()
                        }]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                num_threads: None,
                namespace_capabilities: None,
                builtin_capabilities: None,
                root_component_url: None,
                component_id_index_path: None,
                ..Default::default()
            },
            PolicyConfigError,
            PolicyConfigError::EmptySourceMoniker
        ),
        invalid_root_component_url => (
            component_internal::Config {
                debug: None,
                trace_provider: None,
                enable_introspection: None,
                list_children_batch_size: None,
                maintain_utc_clock: None,
                use_builtin_process_launcher: None,
                security_policy: None,
                num_threads: None,
                namespace_capabilities: None,
                builtin_capabilities: None,
                root_component_url: Some("invalid url".to_string()),
                component_id_index_path: None,
                ..Default::default()
            },
            ParseError,
            ParseError::InvalidComponentUrl {
                details: String::from("Relative URL has no resource fragment.")
            }
        ),
    }

    #[test]
    fn new_from_bytes_valid() -> Result<(), Error> {
        let config = component_internal::Config {
            debug: None,
            trace_provider: None,
            enable_introspection: None,
            list_children_batch_size: Some(42),
            security_policy: None,
            namespace_capabilities: None,
            builtin_capabilities: None,
            maintain_utc_clock: None,
            use_builtin_process_launcher: None,
            num_threads: None,
            root_component_url: Some(FOO_PKG_URL.to_string()),
            ..Default::default()
        };
        let bytes = fidl::persist(&config)?;
        let expected = RuntimeConfig {
            list_children_batch_size: 42,
            root_component_url: Some(Url::new(FOO_PKG_URL.to_string())?),
            ..Default::default()
        };

        assert_matches!(
            RuntimeConfig::new_from_bytes(&bytes)
            , Ok(v) if v == expected);
        Ok(())
    }

    #[test]
    fn new_from_bytes_invalid() -> Result<(), Error> {
        let bytes = vec![0xfa, 0xde];
        assert_matches!(RuntimeConfig::new_from_bytes(&bytes), Err(_));
        Ok(())
    }

    #[test]
    fn abi_revision_policy_check_compatibility_empty_allowlist() -> Result<(), Error> {
        const UNKNOWN_ABI: AbiRevision = AbiRevision::from_u64(0x404);
        const RETIRED_ABI: AbiRevision = AbiRevision::from_u64(0x15);
        const SUPPORTED_ABI: AbiRevision = AbiRevision::from_u64(0x16);

        const VERSIONS: &[Version] = &[
            Version {
                api_level: ApiLevel::from_u32(5),
                abi_revision: RETIRED_ABI,
                status: version_history::Status::Unsupported,
            },
            Version {
                api_level: ApiLevel::from_u32(6),
                abi_revision: SUPPORTED_ABI,
                status: version_history::Status::Supported,
            },
        ];
        let version_history = VersionHistory::new(&VERSIONS);

        let policy = AbiRevisionPolicy::new(vec![]);

        assert_eq!(
            policy.check_compatibility(&version_history, &Moniker::parse_str("/foo")?, None),
            Err(CompatibilityCheckError::AbiRevisionAbsent)
        );
        assert_eq!(
            policy.check_compatibility(
                &version_history,
                &Moniker::parse_str("/foo")?,
                Some(UNKNOWN_ABI)
            ),
            Err(CompatibilityCheckError::AbiRevisionInvalid(AbiRevisionError::TooNew {
                abi_revision: UNKNOWN_ABI,
                supported_versions: VersionVec(vec![VERSIONS[1].clone()])
            }))
        );
        assert_eq!(
            policy.check_compatibility(
                &version_history,
                &Moniker::parse_str("/foo")?,
                Some(RETIRED_ABI)
            ),
            Err(CompatibilityCheckError::AbiRevisionInvalid(AbiRevisionError::Retired {
                version: VERSIONS[0].clone(),
                supported_versions: VersionVec(vec![VERSIONS[1].clone()]),
            })),
        );
        assert_eq!(
            policy.check_compatibility(
                &version_history,
                &Moniker::parse_str("/foo")?,
                Some(SUPPORTED_ABI)
            ),
            Ok(())
        );

        Ok(())
    }

    #[test]
    fn abi_revision_policy_check_compatibility_allowlist() -> Result<(), Error> {
        const UNKNOWN_ABI: AbiRevision = AbiRevision::from_u64(0x404);
        const RETIRED_ABI: AbiRevision = AbiRevision::from_u64(0x15);
        const SUPPORTED_ABI: AbiRevision = AbiRevision::from_u64(0x16);

        const VERSIONS: &[Version] = &[
            Version {
                api_level: ApiLevel::from_u32(5),
                abi_revision: RETIRED_ABI,
                status: version_history::Status::Unsupported,
            },
            Version {
                api_level: ApiLevel::from_u32(6),
                abi_revision: SUPPORTED_ABI,
                status: version_history::Status::Supported,
            },
        ];
        let version_history = VersionHistory::new(&VERSIONS);

        let policy = AbiRevisionPolicy::new(vec![
            AllowlistEntryBuilder::new().exact("foo").any_child().build(),
        ]);

        // "/bar" isn't on the allowlist, so bad usage should fail.
        assert_eq!(
            policy.check_compatibility(&version_history, &Moniker::parse_str("/bar")?, None),
            Err(CompatibilityCheckError::AbiRevisionAbsent)
        );
        assert_eq!(
            policy.check_compatibility(
                &version_history,
                &Moniker::parse_str("/bar")?,
                Some(UNKNOWN_ABI)
            ),
            Err(CompatibilityCheckError::AbiRevisionInvalid(AbiRevisionError::TooNew {
                abi_revision: UNKNOWN_ABI,
                supported_versions: VersionVec(vec![VERSIONS[1].clone()])
            }))
        );
        assert_eq!(
            policy.check_compatibility(
                &version_history,
                &Moniker::parse_str("/bar")?,
                Some(RETIRED_ABI)
            ),
            Err(CompatibilityCheckError::AbiRevisionInvalid(AbiRevisionError::Retired {
                version: VERSIONS[0].clone(),
                supported_versions: VersionVec(vec![VERSIONS[1].clone()]),
            })),
        );
        assert_eq!(
            policy.check_compatibility(
                &version_history,
                &Moniker::parse_str("/bar")?,
                Some(SUPPORTED_ABI)
            ),
            Ok(())
        );

        // "/foo/baz" is on the allowlist. Allow whatever.
        assert_eq!(
            policy.check_compatibility(&version_history, &Moniker::parse_str("/foo/baz")?, None),
            Ok(())
        );
        assert_eq!(
            policy.check_compatibility(
                &version_history,
                &Moniker::parse_str("/foo/baz")?,
                Some(UNKNOWN_ABI)
            ),
            Ok(())
        );
        assert_eq!(
            policy.check_compatibility(
                &version_history,
                &Moniker::parse_str("/foo/baz")?,
                Some(RETIRED_ABI)
            ),
            Ok(())
        );
        assert_eq!(
            policy.check_compatibility(
                &version_history,
                &Moniker::parse_str("/foo/baz")?,
                Some(SUPPORTED_ABI)
            ),
            Ok(())
        );

        Ok(())
    }

    test_function_ok! { parse_allowlist_entries, missing_entries => (&None, vec![]) }

    macro_rules! test_entries_ok {
        ( $($test_name:ident => ($input:expr, $expected:expr)),+ $(,)? ) => {
            $(
                #[test]
                fn $test_name() {
                    let input = $input;
                    let parsed = parse_allowlist_entries(&Some(input.clone())).unwrap();
                    assert_eq!(parsed, $expected);
                    let round_trip: Vec<_> = parsed.iter().map(AllowlistEntry::to_string).collect();
                    assert_eq!(round_trip, input);
                }
            )+
        };
    }

    macro_rules! test_entries_err {
        ( $($test_name:ident => ($input:expr, $type:ty, $expected:expr)),+ $(,)? ) => {
            test_function_err! { parse_allowlist_entries, $($test_name => (&Some($input), $type, $expected)),+ }
        };
    }

    test_entries_ok! {
        empty_entries => (vec![], vec![]),
        all_entry_types => (vec![
            "/core".into(),
            "/**".into(),
            "/foo/**".into(),
            "/coll:**".into(),
            "/core/test_manager/tests:**".into(),
            "/core/ffx-laboratory:*/echo_client".into(),
            "/core/*/ffx-laboratory:*/**".into(),
            "/core/*/bar".into(),
        ], vec![
            AllowlistEntryBuilder::new().exact("core").build(),
            AllowlistEntryBuilder::new().any_descendant(),
            AllowlistEntryBuilder::new().exact("foo").any_descendant(),
            AllowlistEntryBuilder::new().any_descendant_in_collection("coll"),
            AllowlistEntryBuilder::new().exact("core").exact("test_manager").any_descendant_in_collection("tests"),
            AllowlistEntryBuilder::new().exact("core").any_child_in_collection("ffx-laboratory").exact("echo_client").build(),
            AllowlistEntryBuilder::new().exact("core").any_child().any_child_in_collection("ffx-laboratory").any_descendant(),
            AllowlistEntryBuilder::new().exact("core").any_child().exact("bar").build(),
        ])
    }

    test_entries_err! {
        invalid_realm_entry => (
            vec!["/foo/**".into(), "bar/**".into()],
            AllowlistEntryParseError,
            AllowlistEntryParseError::NoLeadingSlash("bar/**".into())),
        invalid_realm_in_collection_entry => (
            vec!["/foo/coll:**".into(), "bar/coll:**".into()],
            AllowlistEntryParseError,
            AllowlistEntryParseError::NoLeadingSlash("bar/coll:**".into())),
        missing_realm_in_collection_entry => (
            vec!["coll:**".into()],
            AllowlistEntryParseError,
            AllowlistEntryParseError::NoLeadingSlash("coll:**".into())),
        missing_collection_name => (
            vec!["/foo/coll:**".into(), "/:**".into()],
            AllowlistEntryParseError,
            AllowlistEntryParseError::InvalidCollectionName(
                "".into(),
                ParseError::Empty
            )),
        invalid_collection_name => (
            vec!["/foo/coll:**".into(), "/*:**".into()],
            AllowlistEntryParseError,
            AllowlistEntryParseError::InvalidCollectionName(
                "*".into(),
                ParseError::InvalidValue
            )),
        invalid_exact_entry => (
            vec!["/foo/bar*".into()],
            AllowlistEntryParseError,
            AllowlistEntryParseError::InvalidChildName(
                "bar*".into(),
                MonikerError::InvalidMonikerPart { 0: ParseError::InvalidValue }
            )),
        descendant_wildcard_in_between => (
            vec!["/foo/**/bar".into()],
            AllowlistEntryParseError,
            AllowlistEntryParseError::DescendantWildcardOnlyAtEnd(
                "/foo/**/bar".into(),
            )),
    }

    #[test]
    fn allowlist_entry_matches() {
        let root = Moniker::root();
        let allowed = Moniker::try_from(["foo", "bar"]).unwrap();
        let disallowed_child_of_allowed = Moniker::try_from(["foo", "bar", "baz"]).unwrap();
        let disallowed = Moniker::try_from(["baz", "fiz"]).unwrap();
        let allowlist_exact = AllowlistEntryBuilder::new().exact_from_moniker(&allowed).build();
        assert!(allowlist_exact.matches(&allowed));
        assert!(!allowlist_exact.matches(&root));
        assert!(!allowlist_exact.matches(&disallowed));
        assert!(!allowlist_exact.matches(&disallowed_child_of_allowed));

        let allowed_realm_root = Moniker::try_from(["qux"]).unwrap();
        let allowed_child_of_realm = Moniker::try_from(["qux", "quux"]).unwrap();
        let allowed_nested_child_of_realm = Moniker::try_from(["qux", "quux", "foo"]).unwrap();
        let allowlist_realm =
            AllowlistEntryBuilder::new().exact_from_moniker(&allowed_realm_root).any_descendant();
        assert!(!allowlist_realm.matches(&allowed_realm_root));
        assert!(allowlist_realm.matches(&allowed_child_of_realm));
        assert!(allowlist_realm.matches(&allowed_nested_child_of_realm));
        assert!(!allowlist_realm.matches(&disallowed));
        assert!(!allowlist_realm.matches(&root));

        let collection_holder = Moniker::try_from(["corge"]).unwrap();
        let collection_child = Moniker::try_from(["corge", "collection:child"]).unwrap();
        let collection_nested_child =
            Moniker::try_from(["corge", "collection:child", "inner-child"]).unwrap();
        let non_collection_child = Moniker::try_from(["corge", "grault"]).unwrap();
        let allowlist_collection = AllowlistEntryBuilder::new()
            .exact_from_moniker(&collection_holder)
            .any_descendant_in_collection("collection");
        assert!(!allowlist_collection.matches(&collection_holder));
        assert!(allowlist_collection.matches(&collection_child));
        assert!(allowlist_collection.matches(&collection_nested_child));
        assert!(!allowlist_collection.matches(&non_collection_child));
        assert!(!allowlist_collection.matches(&disallowed));
        assert!(!allowlist_collection.matches(&root));

        let collection_a = Moniker::try_from(["foo", "bar:a", "baz", "qux"]).unwrap();
        let collection_b = Moniker::try_from(["foo", "bar:b", "baz", "qux"]).unwrap();
        let parent_not_allowed = Moniker::try_from(["foo", "bar:b", "baz"]).unwrap();
        let collection_not_allowed = Moniker::try_from(["foo", "bar:b", "baz"]).unwrap();
        let different_collection_not_allowed =
            Moniker::try_from(["foo", "test:b", "baz", "qux"]).unwrap();
        let allowlist_exact_in_collection = AllowlistEntryBuilder::new()
            .exact("foo")
            .any_child_in_collection("bar")
            .exact("baz")
            .exact("qux")
            .build();
        assert!(allowlist_exact_in_collection.matches(&collection_a));
        assert!(allowlist_exact_in_collection.matches(&collection_b));
        assert!(!allowlist_exact_in_collection.matches(&parent_not_allowed));
        assert!(!allowlist_exact_in_collection.matches(&collection_not_allowed));
        assert!(!allowlist_exact_in_collection.matches(&different_collection_not_allowed));

        let any_child_allowlist = AllowlistEntryBuilder::new().exact("core").any_child().build();
        let allowed = Moniker::try_from(["core", "abc"]).unwrap();
        let disallowed_1 = Moniker::try_from(["not_core", "abc"]).unwrap();
        let disallowed_2 = Moniker::try_from(["core", "abc", "def"]).unwrap();
        assert!(any_child_allowlist.matches(&allowed));
        assert!(!any_child_allowlist.matches(&disallowed_1));
        assert!(!any_child_allowlist.matches(&disallowed_2));

        let multiwildcard_allowlist = AllowlistEntryBuilder::new()
            .exact("core")
            .any_child()
            .any_child_in_collection("foo")
            .any_descendant();
        let allowed = Moniker::try_from(["core", "abc", "foo:def", "ghi"]).unwrap();
        let disallowed_1 = Moniker::try_from(["not_core", "abc", "foo:def", "ghi"]).unwrap();
        let disallowed_2 = Moniker::try_from(["core", "abc", "not_foo:def", "ghi"]).unwrap();
        let disallowed_3 = Moniker::try_from(["core", "abc", "foo:def"]).unwrap();
        assert!(multiwildcard_allowlist.matches(&allowed));
        assert!(!multiwildcard_allowlist.matches(&disallowed_1));
        assert!(!multiwildcard_allowlist.matches(&disallowed_2));
        assert!(!multiwildcard_allowlist.matches(&disallowed_3));
    }
}
