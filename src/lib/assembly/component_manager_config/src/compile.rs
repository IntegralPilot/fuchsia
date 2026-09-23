// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::FromArgs;
use cm_types::{Url, symmetrical_enums};
use cml::error::{Error, Location};
use cml::translate::CompileOptions;
use cml::types::capability::{Capability, span_capability};
use cml::types::capability_id::CapabilityId;
use fidl::persist;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_component_internal as component_internal;
use itertools::Itertools;
use serde::Deserialize;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;

trait Mergeable {
    fn merge(self, another: Self) -> Result<Self, Error>
    where
        Self: Sized;
}

impl<T> Mergeable for Vec<T> {
    fn merge(mut self, another: Self) -> Result<Self, Error> {
        self.extend(another);
        Ok(self)
    }
}

fn remove_duplicates<T: Ord>(mut v: Vec<T>) -> Vec<T> {
    v.sort();
    v.dedup();
    v
}

fn merge_option<T, F>(left: Option<T>, right: Option<T>, merge_fn: F) -> Result<Option<T>, Error>
where
    F: FnOnce(T, T) -> Result<T, Error>,
{
    match (left, right) {
        (left @ Some(_), None) => Ok(left),
        (None, right @ Some(_)) => Ok(right),
        (Some(l), Some(r)) => Ok(Some(merge_fn(l, r)?)),
        (None, None) => Ok(None),
    }
}

macro_rules! deep_merge_field {
    ($target:ident, $other:expr, $field:ident) => {
        merge_option($target.$field, $other.$field, |l, r| l.merge(r))?
    };
}

macro_rules! merge_field {
    ($target:ident, $other:expr, $field:ident) => {
        merge_option($target.$field, $other.$field, |_, _| {
            Err(Error::parse(
                format!("Conflicting field found: {:?}", stringify!($field)),
                None,
                None,
            ))
        })?
    };
}

macro_rules! merge_vec {
    ($target:ident, $other:expr, $field:ident) => {
        merge_option($target.$field, $other.$field, |mut l, mut r| {
            l.append(&mut r);
            Ok(remove_duplicates(l))
        })?
    };
}

const SESSION_MONIKER: &str = "/core/session-manager/session:session";

/// We use types for the platform capabilities so that the product owners get a nice error
/// indicating their options if they choose an invalid capability.
#[derive(Deserialize, Debug, Eq, PartialEq, Hash, Clone)]
enum PlatformCapability {
    #[serde(rename = "fuchsia.lowpan.device.DeviceExtraConnector")]
    Lowpan,
    #[serde(rename = "fuchsia.kernel.VmexResource")]
    VmexResource,
    #[serde(rename = "fuchsia.pkg.PackageCache")]
    PackageCache,
}

impl PlatformCapability {
    /// Convert the capability back to a string.
    fn source_name(&self) -> &str {
        match self {
            &PlatformCapability::Lowpan => "fuchsia.lowpan.device.DeviceExtraConnector",
            &PlatformCapability::VmexResource => "fuchsia.kernel.VmexResource",
            &PlatformCapability::PackageCache => "fuchsia.pkg.PackageCache",
        }
    }

    /// Map the capability to the moniker of the source.
    fn source_moniker(&self) -> &str {
        match self {
            &PlatformCapability::Lowpan => "/core/lowpanservice",
            &PlatformCapability::VmexResource => "<component_manager>",
            &PlatformCapability::PackageCache => "/bootstrap/pkg-cache",
        }
    }
}

#[derive(Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
struct Config {
    debug: Option<bool>,
    trace_provider: Option<TraceProvider>,
    list_children_batch_size: Option<u32>,
    security_policy: Option<SecurityPolicy>,
    namespace_capabilities: Option<Vec<Capability>>,
    builtin_capabilities: Option<Vec<Capability>>,
    use_builtin_process_launcher: Option<bool>,
    maintain_utc_clock: Option<bool>,
    num_threads: Option<u8>,
    root_component_url: Option<Url>,
    component_id_index_path: Option<String>,
    log_destination: Option<LogDestination>,
    log_all_events: Option<bool>,
    builtin_boot_resolver: Option<BuiltinBootResolver>,
    realm_builder_resolver_and_runner: Option<RealmBuilderResolverAndRunner>,
    enable_introspection: Option<bool>,
    abi_revision_policy: Option<AbiRevisionPolicy>,
    vmex_source: Option<VmexSource>,
    health_check: Option<HealthCheck>,
    inject_capabilities: Option<Vec<cm_config::InjectedCapabilities>>,
    scudo_options: Option<String>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
struct HealthCheck {
    pub monikers: Option<Vec<String>>,
}

impl Into<component_internal::HealthCheck> for HealthCheck {
    fn into(self) -> component_internal::HealthCheck {
        component_internal::HealthCheck {
            monikers: self.monikers,
            ..component_internal::HealthCheck::default()
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
enum LogDestination {
    Syslog,
    Klog,
}

symmetrical_enums!(LogDestination, component_internal::LogDestination, Syslog, Klog);

#[derive(Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
enum BuiltinBootResolver {
    None,
    Boot,
}

symmetrical_enums!(BuiltinBootResolver, component_internal::BuiltinBootResolver, None, Boot);

#[derive(Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
enum RealmBuilderResolverAndRunner {
    None,
    Namespace,
}

symmetrical_enums!(
    RealmBuilderResolverAndRunner,
    component_internal::RealmBuilderResolverAndRunner,
    None,
    Namespace
);

#[derive(Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct AbiRevisionPolicy {
    allowlist: Option<Vec<String>>,
}

impl Into<component_internal::AbiRevisionPolicy> for AbiRevisionPolicy {
    fn into(self) -> component_internal::AbiRevisionPolicy {
        component_internal::AbiRevisionPolicy {
            allowlist: self.allowlist,
            ..component_internal::AbiRevisionPolicy::default()
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum VmexSource {
    SystemResource,
    Namespace,
}

symmetrical_enums!(VmexSource, component_internal::VmexSource, SystemResource, Namespace);

#[derive(Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum TraceProvider {
    Namespace,
    RootExposed,
}

symmetrical_enums!(TraceProvider, component_internal::TraceProvider, Namespace, RootExposed);

#[derive(Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct SecurityPolicy {
    job_policy: Option<JobPolicyAllowlists>,
    capability_policy: Option<Vec<CapabilityAllowlistEntry>>,
    debug_registration_policy: Option<Vec<DebugRegistrationAllowlistEntry>>,
    child_policy: Option<ChildPolicyAllowlists>,
}

impl Mergeable for SecurityPolicy {
    fn merge(self, another: Self) -> Result<Self, Error> {
        return Ok(Self {
            job_policy: deep_merge_field!(self, another, job_policy),
            capability_policy: merge_option(
                self.capability_policy,
                another.capability_policy,
                CapabilityAllowlistEntry::merge_vecs,
            )?,
            debug_registration_policy: merge_field!(self, another, debug_registration_policy),
            child_policy: deep_merge_field!(self, another, child_policy),
        });
    }
}

#[derive(Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct JobPolicyAllowlists {
    ambient_mark_vmo_exec: Option<Vec<String>>,
    main_process_critical: Option<Vec<String>>,
    create_raw_processes: Option<Vec<String>>,
}

impl JobPolicyAllowlists {
    fn merge(self, another: JobPolicyAllowlists) -> Result<Self, Error> {
        return Ok(JobPolicyAllowlists {
            ambient_mark_vmo_exec: merge_vec!(self, another, ambient_mark_vmo_exec),
            main_process_critical: merge_vec!(self, another, main_process_critical),
            create_raw_processes: merge_vec!(self, another, create_raw_processes),
        });
    }
}

#[derive(Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct ChildPolicyAllowlists {
    reboot_on_terminate: Option<Vec<String>>,
}

impl ChildPolicyAllowlists {
    fn merge(self, another: ChildPolicyAllowlists) -> Result<Self, Error> {
        return Ok(ChildPolicyAllowlists {
            reboot_on_terminate: merge_vec!(self, another, reboot_on_terminate),
        });
    }
}

#[derive(Deserialize, Debug, Clone, PartialEq, Hash, Eq, Ord, PartialOrd)]
#[serde(rename_all = "lowercase")]
pub enum CapabilityTypeName {
    Directory,
    Protocol,
    Service,
    Storage,
    Runner,
    Resolver,
}

impl std::fmt::Display for CapabilityTypeName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let display_name = match &self {
            CapabilityTypeName::Directory => "directory",
            CapabilityTypeName::Protocol => "protocol",
            CapabilityTypeName::Service => "service",
            CapabilityTypeName::Storage => "storage",
            CapabilityTypeName::Runner => "runner",
            CapabilityTypeName::Resolver => "resolver",
        };
        write!(f, "{}", display_name)
    }
}

impl Into<component_internal::AllowlistedCapability> for CapabilityTypeName {
    fn into(self) -> component_internal::AllowlistedCapability {
        match &self {
            CapabilityTypeName::Directory => component_internal::AllowlistedCapability::Directory(
                component_internal::AllowlistedDirectory::default(),
            ),
            CapabilityTypeName::Protocol => component_internal::AllowlistedCapability::Protocol(
                component_internal::AllowlistedProtocol::default(),
            ),
            CapabilityTypeName::Service => component_internal::AllowlistedCapability::Service(
                component_internal::AllowlistedService::default(),
            ),
            CapabilityTypeName::Storage => component_internal::AllowlistedCapability::Storage(
                component_internal::AllowlistedStorage::default(),
            ),
            CapabilityTypeName::Runner => component_internal::AllowlistedCapability::Runner(
                component_internal::AllowlistedRunner::default(),
            ),
            CapabilityTypeName::Resolver => component_internal::AllowlistedCapability::Resolver(
                component_internal::AllowlistedResolver::default(),
            ),
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "lowercase")]
pub enum DebugRegistrationTypeName {
    Protocol,
}

impl Into<component_internal::AllowlistedDebugRegistration> for DebugRegistrationTypeName {
    fn into(self) -> component_internal::AllowlistedDebugRegistration {
        match &self {
            DebugRegistrationTypeName::Protocol => {
                component_internal::AllowlistedDebugRegistration::Protocol(
                    component_internal::AllowlistedProtocol::default(),
                )
            }
        }
    }
}

#[derive(Deserialize, Debug, Clone, PartialEq, Hash, Eq, Ord, PartialOrd)]
#[serde(rename_all = "lowercase")]
pub enum CapabilityFrom {
    Capability,
    Component,
    Framework,
}

impl Into<fdecl::Ref> for CapabilityFrom {
    fn into(self) -> fdecl::Ref {
        match &self {
            CapabilityFrom::Capability => {
                fdecl::Ref::Capability(fdecl::CapabilityRef { name: "".into() })
            }
            CapabilityFrom::Component => fdecl::Ref::Self_(fdecl::SelfRef {}),
            CapabilityFrom::Framework => fdecl::Ref::Framework(fdecl::FrameworkRef {}),
        }
    }
}

#[derive(Deserialize, Debug, Default, PartialEq, Clone, Ord, PartialOrd, Eq)]
#[serde(deny_unknown_fields)]
pub struct CapabilityAllowlistEntry {
    source_moniker: Option<String>,
    source_name: Option<String>,
    source: Option<CapabilityFrom>,
    capability: Option<CapabilityTypeName>,
    target_monikers: Option<Vec<String>>,
}

impl CapabilityAllowlistEntry {
    fn merge_vecs(
        some: Vec<CapabilityAllowlistEntry>,
        another: Vec<CapabilityAllowlistEntry>,
    ) -> Result<Vec<CapabilityAllowlistEntry>, Error> {
        #[derive(Hash, Eq, PartialEq)]
        struct Source {
            source_moniker: Option<String>,
            source_name: Option<String>,
            source: Option<CapabilityFrom>,
            capability: Option<CapabilityTypeName>,
        }
        let mut combined: Vec<_> = some
            .into_iter()
            .chain(another.into_iter())
            .map(|mut entry| {
                (
                    Source {
                        source_moniker: entry.source_moniker.take(),
                        source_name: entry.source_name.take(),
                        source: entry.source.take(),
                        capability: entry.capability.take(),
                    },
                    entry.target_monikers,
                )
            })
            .into_grouping_map()
            .reduce(|accumulated, _key, item| {
                {
                    merge_option(accumulated, item, |mut l, mut r| {
                        l.append(&mut r);
                        Ok(l)
                    })
                }
                .unwrap()
            })
            .into_iter()
            .map(|(mut key, values)| CapabilityAllowlistEntry {
                source_moniker: key.source_moniker.take(),
                source_name: key.source_name.take(),
                source: key.source.take(),
                capability: key.capability.take(),
                target_monikers: match values {
                    Some(v) => Some(remove_duplicates(v)),
                    None => None,
                },
            })
            .collect();
        // Unstable sorts are faster, and we shouldn't have items that have equal values anyway.
        combined.sort_unstable();
        Ok(combined)
    }
}

#[derive(Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct DebugRegistrationAllowlistEntry {
    name: Option<String>,
    debug: Option<DebugRegistrationTypeName>,
    moniker: Option<String>,
    environment_name: Option<String>,
}

impl TryFrom<Config> for component_internal::Config {
    type Error = Error;

    fn try_from(config: Config) -> Result<Self, Error> {
        // Validate "namespace_capabilities".
        if let Some(capabilities) = config.namespace_capabilities.as_ref() {
            let mut used_ids = HashSet::new();
            for capability in capabilities {
                Config::validate_namespace_capability(capability, &mut used_ids)?;
            }
        }
        // Validate "builtin_capabilities".
        if let Some(capabilities) = config.builtin_capabilities.as_ref() {
            let mut used_ids = HashSet::new();
            for capability in capabilities {
                Config::validate_builtin_capability(capability, &mut used_ids)?;
            }
        }

        Ok(Self {
            debug: config.debug,
            trace_provider: config.trace_provider.map(Into::into),
            enable_introspection: config.enable_introspection,
            use_builtin_process_launcher: config.use_builtin_process_launcher,
            maintain_utc_clock: config.maintain_utc_clock,
            list_children_batch_size: config.list_children_batch_size,
            security_policy: Some(translate_security_policy(config.security_policy)),
            namespace_capabilities: config
                .namespace_capabilities
                .as_ref()
                .map(|capabilities| {
                    let spanned_caps: Vec<_> =
                        capabilities.iter().cloned().map(span_capability).collect();

                    cml::translate::translate_capabilities(
                        &CompileOptions::default(),
                        &spanned_caps,
                        false,
                    )
                })
                .transpose()?,
            builtin_capabilities: config
                .builtin_capabilities
                .as_ref()
                .map(|capabilities| {
                    let spanned_caps: Vec<_> =
                        capabilities.iter().cloned().map(span_capability).collect();

                    cml::translate::translate_capabilities(
                        &CompileOptions::default(),
                        &spanned_caps,
                        true,
                    )
                })
                .transpose()?,
            num_threads: config.num_threads,
            root_component_url: match config.root_component_url {
                Some(root_component_url) => Some(root_component_url.as_str().to_string()),
                None => None,
            },
            component_id_index_path: config.component_id_index_path,
            log_destination: config.log_destination.map(|d| d.into()),
            log_all_events: config.log_all_events,
            builtin_boot_resolver: config.builtin_boot_resolver.map(Into::into),
            realm_builder_resolver_and_runner: config
                .realm_builder_resolver_and_runner
                .map(Into::into),
            abi_revision_policy: config.abi_revision_policy.map(Into::into),
            vmex_source: config.vmex_source.map(Into::into),
            health_check: config.health_check.map(Into::into),
            inject_capabilities: Some(
                config
                    .inject_capabilities
                    .unwrap_or_default()
                    .into_iter()
                    .map(Into::into)
                    .collect(),
            ),
            scudo_options: config.scudo_options,
            ..Default::default()
        })
    }
}

// TODO: Instead of returning a "default" security_policy when it's not specified in JSON,
// could we return None instead?
fn translate_security_policy(
    security_policy: Option<SecurityPolicy>,
) -> component_internal::SecurityPolicy {
    let SecurityPolicy { job_policy, capability_policy, debug_registration_policy, child_policy } =
        security_policy.unwrap_or_default();
    component_internal::SecurityPolicy {
        job_policy: job_policy.map(translate_job_policy),
        capability_policy: capability_policy.map(translate_capability_policy),
        debug_registration_policy: debug_registration_policy
            .map(translate_debug_registration_policy),
        child_policy: child_policy.map(translate_child_policy),
        ..Default::default()
    }
}

fn translate_job_policy(
    job_policy: JobPolicyAllowlists,
) -> component_internal::JobPolicyAllowlists {
    component_internal::JobPolicyAllowlists {
        ambient_mark_vmo_exec: job_policy.ambient_mark_vmo_exec,
        main_process_critical: job_policy.main_process_critical,
        create_raw_processes: job_policy.create_raw_processes,
        ..Default::default()
    }
}

fn translate_child_policy(
    child_policy: ChildPolicyAllowlists,
) -> component_internal::ChildPolicyAllowlists {
    component_internal::ChildPolicyAllowlists {
        reboot_on_terminate: child_policy.reboot_on_terminate,
        ..Default::default()
    }
}

fn translate_capability_policy(
    capability_policy: Vec<CapabilityAllowlistEntry>,
) -> component_internal::CapabilityPolicyAllowlists {
    let allowlist = capability_policy
        .iter()
        .map(|e| component_internal::CapabilityAllowlistEntry {
            source_moniker: e.source_moniker.clone(),
            source_name: e.source_name.clone(),
            source: e.source.clone().map_or_else(|| None, |t| Some(t.into())),
            capability: e.capability.clone().map_or_else(|| None, |t| Some(t.into())),
            target_monikers: e.target_monikers.clone(),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    component_internal::CapabilityPolicyAllowlists {
        allowlist: Some(allowlist),
        ..Default::default()
    }
}

fn translate_debug_registration_policy(
    debug_registration_policy: Vec<DebugRegistrationAllowlistEntry>,
) -> component_internal::DebugRegistrationPolicyAllowlists {
    let allowlist = debug_registration_policy
        .iter()
        .map(|e| component_internal::DebugRegistrationAllowlistEntry {
            name: e.name.clone(),
            debug: e.debug.clone().map_or_else(|| None, |t| Some(t.into())),
            moniker: e.moniker.clone(),
            environment_name: e.environment_name.clone(),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    component_internal::DebugRegistrationPolicyAllowlists {
        allowlist: Some(allowlist),
        ..Default::default()
    }
}

impl Config {
    fn from_json_file(path: &PathBuf) -> Result<Self, Error> {
        let data = fs::read_to_string(path)?;
        serde_json5::from_str(&data).map_err(|e| {
            let serde_json5::Error::Message { location, msg } = e;
            let location = location.map(|l| Location { line: l.line, column: l.column });
            Error::parse(msg, location, Some(path))
        })
    }

    fn merge(self, another: Config) -> Result<Self, Error> {
        Ok(Config {
            debug: merge_field!(self, another, debug),
            trace_provider: merge_field!(self, another, trace_provider),
            enable_introspection: merge_field!(self, another, enable_introspection),
            use_builtin_process_launcher: merge_field!(self, another, use_builtin_process_launcher),
            maintain_utc_clock: merge_field!(self, another, maintain_utc_clock),
            list_children_batch_size: merge_field!(self, another, list_children_batch_size),
            security_policy: deep_merge_field!(self, another, security_policy),
            namespace_capabilities: deep_merge_field!(self, another, namespace_capabilities),
            builtin_capabilities: deep_merge_field!(self, another, builtin_capabilities),
            num_threads: merge_field!(self, another, num_threads),
            root_component_url: merge_field!(self, another, root_component_url),
            component_id_index_path: merge_field!(self, another, component_id_index_path),
            log_destination: merge_field!(self, another, log_destination),
            log_all_events: merge_field!(self, another, log_all_events),
            builtin_boot_resolver: merge_field!(self, another, builtin_boot_resolver),
            realm_builder_resolver_and_runner: merge_field!(
                self,
                another,
                realm_builder_resolver_and_runner
            ),
            abi_revision_policy: merge_field!(self, another, abi_revision_policy),
            vmex_source: merge_field!(self, another, vmex_source),
            health_check: merge_field!(self, another, health_check),
            inject_capabilities: deep_merge_field!(self, another, inject_capabilities),
            scudo_options: merge_field!(self, another, scudo_options),
        })
    }

    fn validate_namespace_capability(
        capability: &Capability,
        used_ids: &mut HashSet<String>,
    ) -> Result<(), Error> {
        if capability.directory.is_some() && capability.path.is_none() {
            return Err(Error::validate("\"path\" should be present with \"directory\""));
        }
        if capability.directory.is_some() && capability.rights.is_none() {
            return Err(Error::validate("\"rights\" should be present with \"directory\""));
        }
        if capability.storage.is_some() {
            return Err(Error::validate("\"storage\" is not supported for namespace capabilities"));
        }
        if capability.runner.is_some() {
            return Err(Error::validate("\"runner\" is not supported for namespace capabilities"));
        }
        if capability.resolver.is_some() {
            return Err(Error::validate(
                "\"resolver\" is not supported for namespace capabilities",
            ));
        }

        // Disallow multiple capability ids of the same name.
        let binding = span_capability(capability.clone());
        let capability_ids = CapabilityId::from_context_capability(&binding)?;
        for (capability_id, _) in capability_ids {
            if !used_ids.insert(capability_id.to_string()) {
                return Err(Error::validate(format!(
                    "\"{}\" is a duplicate \"capability\" name",
                    capability_id,
                )));
            }
        }

        Ok(())
    }

    fn validate_builtin_capability(
        capability: &Capability,
        used_ids: &mut HashSet<String>,
    ) -> Result<(), Error> {
        if capability.storage.is_some() {
            return Err(Error::validate("\"storage\" is not supported for built-in capabilities"));
        }
        if capability.directory.is_some() && capability.rights.is_none() {
            return Err(Error::validate("\"rights\" should be present with \"directory\""));
        }
        if capability.path.is_some() {
            return Err(Error::validate(
                "\"path\" should not be present for built-in capabilities",
            ));
        }

        // Disallow multiple capability ids of the same name.
        let binding = span_capability(capability.clone());
        let capability_ids = CapabilityId::from_context_capability(&binding)?;
        for (capability_id, _) in capability_ids {
            if !used_ids.insert(capability_id.to_string()) {
                return Err(Error::validate(format!(
                    "\"{}\" is a duplicate \"capability\" name",
                    capability_id,
                )));
            }
        }

        Ok(())
    }
}

#[derive(Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
struct ProductConfig {
    #[serde(default)]
    ambient_mark_vmo_exec: AmbientMarkVmoExec,
    #[serde(default)]
    platform_capabilities: PlatformCapabilities,
    #[serde(default)]
    product_capabilities: ProductCapabilities,
}

#[derive(Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
struct AmbientMarkVmoExec {
    session: Vec<String>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
struct PlatformCapabilities {
    session: Vec<PlatformCapabilityEntry>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
struct ProductCapabilities {
    session: Vec<ProductCapabilityEntry>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct PlatformCapabilityEntry {
    capability: PlatformCapability,
    target_monikers: Vec<String>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct ProductCapabilityEntry {
    source_moniker: String,
    capability: String,
    target_monikers: Vec<String>,
}

impl ProductConfig {
    fn from_json_file(path: &PathBuf) -> Result<Self, Error> {
        let data = fs::read_to_string(path)?;
        serde_json5::from_str(&data).map_err(|e| {
            let serde_json5::Error::Message { location, msg } = e;
            let location = location.map(|l| Location { line: l.line, column: l.column });
            Error::parse(msg, location, Some(path))
        })
    }
}

impl TryInto<Config> for ProductConfig {
    type Error = Error;

    fn try_into(self) -> Result<Config, Error> {
        let Self { ambient_mark_vmo_exec, platform_capabilities, product_capabilities } = self;

        fn rebase_string(s: String) -> String {
            format!("{SESSION_MONIKER}/{s}")
        }

        fn rebase_vec_string(vec: Vec<String>) -> Vec<String> {
            vec.into_iter().map(|s| rebase_string(s)).collect()
        }

        let vmo_exec =
            ambient_mark_vmo_exec.session.into_iter().map(|s| rebase_string(s)).collect();
        let job_policy =
            JobPolicyAllowlists { ambient_mark_vmo_exec: Some(vmo_exec), ..Default::default() };

        let platform_capabilities =
            platform_capabilities.session.into_iter().map(|entry| CapabilityAllowlistEntry {
                source: Some(CapabilityFrom::Component),
                capability: Some(CapabilityTypeName::Protocol),
                source_moniker: Some(entry.capability.source_moniker().to_string()),
                source_name: Some(entry.capability.source_name().to_string()),
                target_monikers: Some(rebase_vec_string(entry.target_monikers)),
            });

        let capability_policy = product_capabilities
            .session
            .into_iter()
            .map(|entry| CapabilityAllowlistEntry {
                source: Some(CapabilityFrom::Component),
                capability: Some(CapabilityTypeName::Protocol),
                source_moniker: Some(rebase_string(entry.source_moniker)),
                source_name: Some(entry.capability),
                target_monikers: Some(rebase_vec_string(entry.target_monikers)),
            })
            .chain(platform_capabilities)
            .collect();

        Ok(Config {
            security_policy: Some(SecurityPolicy {
                job_policy: Some(job_policy),
                capability_policy: Some(capability_policy),
                ..Default::default()
            }),
            ..Default::default()
        })
    }
}

/// Create a binary config and populate it with data from .json file.
#[derive(Debug, Default, FromArgs)]
pub struct Args {
    /// path to a JSON configuration file
    #[argh(option)]
    pub input: Vec<PathBuf>,

    /// path to a product-specific JSON configuration file
    #[argh(option)]
    pub product: Vec<PathBuf>,

    /// path to the output binary config file
    #[argh(option)]
    pub output: PathBuf,
}

/// Compile multiple platform configs with an optional product config, and generate a file that can
/// be added to BootFS for component_manager to use at runtime.
pub fn compile(args: Args) -> Result<(), Error> {
    let configs =
        args.input.iter().map(Config::from_json_file).collect::<Result<Vec<Config>, _>>()?;
    let mut config_json =
        configs.into_iter().try_fold(Config::default(), |acc, next| acc.merge(next))?;

    for product in args.product.into_iter() {
        let product = ProductConfig::from_json_file(&product)?;
        config_json = config_json.merge(product.try_into()?)?;
    }

    let config_fidl: component_internal::Config = config_json.try_into()?;
    let bytes = persist(&config_fidl).map_err(|e| Error::FidlEncoding(e))?;
    let mut file = File::create(args.output).map_err(|e| Error::Io(e))?;
    file.write_all(&bytes).map_err(|e| Error::Io(e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fidl::unpersist;
    use fidl_fuchsia_component_decl as fdecl;
    use fidl_fuchsia_io as fio;
    use std::io::Read;
    use tempfile::TempDir;

    fn compile_str(input: &str) -> Result<component_internal::Config, Error> {
        let tmp_dir = TempDir::new().unwrap();
        let input_path = tmp_dir.path().join("config.json");
        let output_path = tmp_dir.path().join("config.fidl");
        File::create(&input_path).unwrap().write_all(input.as_bytes()).unwrap();
        let args = Args { output: output_path.clone(), input: vec![input_path], product: vec![] };
        compile(args)?;
        let mut bytes = Vec::new();
        File::open(output_path)?.read_to_end(&mut bytes)?;
        let config: component_internal::Config = unpersist(&bytes)?;
        Ok(config)
    }

    #[test]
    fn test_compile() {
        let input = r#"{
            debug: true,
            trace_provider: "namespace",
            enable_introspection: true,
            list_children_batch_size: 123,
            maintain_utc_clock: false,
            use_builtin_process_launcher: true,
            security_policy: {
                job_policy: {
                    main_process_critical: [ "/", "/bar" ],
                    ambient_mark_vmo_exec: ["/foo"],
                    create_raw_processes: ["/baz"],
                },
                capability_policy: [
                    {
                        source_moniker: "<component_manager>",
                        source: "component",
                        source_name: "fuchsia.kernel.MmioResource",
                        capability: "protocol",
                        target_monikers: ["/root", "/root/bootstrap", "/root/core"],
                    },
                ],
                debug_registration_policy: [
                    {
                        name: "fuchsia.kernel.MmioResource",
                        debug: "protocol",
                        moniker: "/foo",
                        environment_name: "my_env",
                    },
                ],
                child_policy: {
                    reboot_on_terminate: [ "/buz" ],
                },
            },
            namespace_capabilities: [
                {
                    protocol: "foo_svc",
                },
                {
                    directory: "bar_dir",
                    path: "/bar",
                    rights: [ "connect" ],
                },
            ],
            builtin_capabilities: [
                {
                    protocol: "foo_protocol",
                },
                {
                    directory: "foo_dir",
                    rights: [ "connect" ],
                },
                {
                    service: "foo_svc",
                },
                {
                    runner: "foo_runner",
                },
                {
                    resolver: "foo_resolver",
                },
                {
                    event_stream: "foo_event_stream",
                }
            ],
            num_threads: 210,
            root_component_url: "fuchsia-pkg://fuchsia.com/foo#meta/foo.cm",
            component_id_index_path: "/this/is/an/absolute/path",
            log_destination: "klog",
            log_all_events: true,
            builtin_boot_resolver: "boot",
            realm_builder_resolver_and_runner: "namespace",
            vmex_source: "namespace",
            health_check: { monikers : ["/block/an/update", "/check/before/committing"]},
            inject_capabilities: [
                {
                    "components": [ "/hello", "/hello/world" ],
                    "use": [
                        {
                            "type": "protocol",
                            "source_name": "fuchsia.foo.bar",
                            "target_path": "/svc/fuchsia.foo.baz",
                        },
                    ],
                },
            ],
            scudo_options: "NAME=value",
        }"#;
        let config = compile_str(input).expect("failed to compile");
        assert_eq!(
            config,
            component_internal::Config {
                debug: Some(true),
                trace_provider: Some(component_internal::TraceProvider::Namespace),
                enable_introspection: Some(true),
                maintain_utc_clock: Some(false),
                use_builtin_process_launcher: Some(true),
                list_children_batch_size: Some(123),
                security_policy: Some(component_internal::SecurityPolicy {
                    job_policy: Some(component_internal::JobPolicyAllowlists {
                        main_process_critical: Some(vec!["/".to_string(), "/bar".to_string()]),
                        ambient_mark_vmo_exec: Some(vec!["/foo".to_string()]),
                        create_raw_processes: Some(vec!["/baz".to_string()]),
                        ..Default::default()
                    }),
                    capability_policy: Some(component_internal::CapabilityPolicyAllowlists {
                        allowlist: Some(vec![component_internal::CapabilityAllowlistEntry {
                            source_moniker: Some("<component_manager>".to_string()),
                            source_name: Some("fuchsia.kernel.MmioResource".to_string()),
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                            capability: Some(component_internal::AllowlistedCapability::Protocol(
                                component_internal::AllowlistedProtocol::default()
                            )),
                            target_monikers: Some(vec![
                                "/root".to_string(),
                                "/root/bootstrap".to_string(),
                                "/root/core".to_string()
                            ]),
                            ..Default::default()
                        },]),
                        ..Default::default()
                    }),
                    debug_registration_policy: Some(
                        component_internal::DebugRegistrationPolicyAllowlists {
                            allowlist: Some(vec![
                                component_internal::DebugRegistrationAllowlistEntry {
                                    name: Some("fuchsia.kernel.MmioResource".to_string()),
                                    debug: Some(
                                        component_internal::AllowlistedDebugRegistration::Protocol(
                                            component_internal::AllowlistedProtocol::default()
                                        )
                                    ),
                                    moniker: Some("/foo".to_string()),
                                    environment_name: Some("my_env".to_string()),
                                    ..Default::default()
                                }
                            ]),
                            ..Default::default()
                        }
                    ),
                    child_policy: Some(component_internal::ChildPolicyAllowlists {
                        reboot_on_terminate: Some(vec!["/buz".to_string()]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                namespace_capabilities: Some(vec![
                    fdecl::Capability::Protocol(fdecl::Protocol {
                        name: Some("foo_svc".into()),
                        source_path: Some("/svc/foo_svc".into()),
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
                    fdecl::Capability::Directory(fdecl::Directory {
                        name: Some("foo_dir".into()),
                        source_path: None,
                        rights: Some(fio::Operations::CONNECT),
                        ..Default::default()
                    }),
                    fdecl::Capability::Service(fdecl::Service {
                        name: Some("foo_svc".into()),
                        source_path: None,
                        ..Default::default()
                    }),
                    fdecl::Capability::Runner(fdecl::Runner {
                        name: Some("foo_runner".into()),
                        source_path: None,
                        ..Default::default()
                    }),
                    fdecl::Capability::Resolver(fdecl::Resolver {
                        name: Some("foo_resolver".into()),
                        source_path: None,
                        ..Default::default()
                    }),
                    fdecl::Capability::EventStream(fdecl::EventStream {
                        name: Some("foo_event_stream".into()),
                        ..Default::default()
                    }),
                ]),
                num_threads: Some(210),
                root_component_url: Some("fuchsia-pkg://fuchsia.com/foo#meta/foo.cm".to_string()),
                component_id_index_path: Some("/this/is/an/absolute/path".to_string()),
                log_destination: Some(component_internal::LogDestination::Klog),
                log_all_events: Some(true),
                builtin_boot_resolver: Some(component_internal::BuiltinBootResolver::Boot),
                realm_builder_resolver_and_runner: Some(
                    component_internal::RealmBuilderResolverAndRunner::Namespace
                ),
                vmex_source: Some(component_internal::VmexSource::Namespace),
                health_check: Some(component_internal::HealthCheck {
                    monikers: Some(vec![
                        "/block/an/update".to_string(),
                        "/check/before/committing".to_string()
                    ]),
                    ..Default::default()
                }),
                inject_capabilities: Some(vec![component_internal::InjectedCapabilities {
                    components: Some(vec!["/hello".to_string(), "/hello/world".to_string()]),
                    use_: Some(vec![component_internal::InjectedUse::Protocol(
                        component_internal::InjectedUseProtocol {
                            source_name: Some("fuchsia.foo.bar".to_string()),
                            target_path: Some("/svc/fuchsia.foo.baz".to_string()),
                            ..Default::default()
                        }
                    )]),
                    ..Default::default()
                }]),
                scudo_options: Some("NAME=value".to_string()),
                ..Default::default()
            }
        );
    }

    #[test]
    fn test_validate_namespace_capabilities() {
        {
            let input = r#"{
            namespace_capabilities: [
                {
                    protocol: "foo",
                },
                {
                    directory: "foo",
                    path: "/foo",
                    rights: [ "connect" ],
                },
            ],
        }"#;
            assert_matches!(
                compile_str(input),
                Err(Error::Validate { err, .. } )
                    if &err == "\"foo\" is a duplicate \"capability\" name"
            );
        }
        {
            let input = r#"{
            namespace_capabilities: [
                {
                    directory: "foo",
                    path: "/foo",
                },
            ],
        }"#;
            assert_matches!(
                compile_str(input),
                Err(Error::Validate { err, .. } )
                    if &err == "\"rights\" should be present with \"directory\""
            );
        }
        {
            let input = r#"{
            namespace_capabilities: [
                {
                    directory: "foo",
                    rights: [ "connect" ],
                },
            ],
        }"#;
            assert_matches!(
                compile_str(input),
                Err(Error::Validate { err, .. } )
                    if &err == "\"path\" should be present with \"directory\""
            );
        }
    }

    #[test]
    fn test_validate_builtin_capabilities() {
        {
            let input = r#"{
            builtin_capabilities: [
                {
                    protocol: "foo",
                },
                {
                    directory: "foo",
                    rights: [ "connect" ],
                },
            ],
        }"#;
            assert_matches!(
                compile_str(input),
                Err(Error::Validate { err, .. } )
                    if &err == "\"foo\" is a duplicate \"capability\" name"
            );
        }
        {
            let input = r#"{
            builtin_capabilities: [
                {
                    directory: "foo",
                    path: "/foo",
                },
            ],
        }"#;
            assert_matches!(
                compile_str(input),
                Err(Error::Validate { err, .. } )
                    if &err == "\"rights\" should be present with \"directory\""
            );
        }
        {
            let input = r#"{
            builtin_capabilities: [
                {
                    storage: "foo",
                },
            ],
        }"#;
            assert_matches!(
                compile_str(input),
                Err(Error::Validate { err, .. } )
                    if &err == "\"storage\" is not supported for built-in capabilities"
            );
        }
        {
            let input = r#"{
            builtin_capabilities: [
                {
                    runner: "foo",
                    path: "/foo",
                },
            ],
        }"#;
            assert_matches!(
                compile_str(input),
                Err(Error::Validate { err, .. } )
                    if &err == "\"path\" should not be present for built-in capabilities"
            );
        }
    }

    #[test]
    fn test_compile_conflict() {
        let tmp_dir = TempDir::new().unwrap();
        let output_path = tmp_dir.path().join("config");

        let input_path = tmp_dir.path().join("foo.json");
        let input = "{\"debug\": true,}";
        File::create(&input_path).unwrap().write_all(input.as_bytes()).unwrap();

        let another_input_path = tmp_dir.path().join("bar.json");
        let another_input = "{\"debug\": false,}";
        File::create(&another_input_path).unwrap().write_all(another_input.as_bytes()).unwrap();

        let args = Args {
            output: output_path.clone(),
            input: vec![input_path, another_input_path],
            product: vec![],
        };
        assert_matches!(compile(args), Err(Error::Parse { .. }));
    }

    #[test]
    fn test_merge() -> Result<(), Error> {
        let tmp_dir = TempDir::new().unwrap();
        let output_path = tmp_dir.path().join("config");

        let input_path = tmp_dir.path().join("foo.json");
        let input = "{\"debug\": true,}";
        File::create(&input_path).unwrap().write_all(input.as_bytes()).unwrap();

        let another_input_path = tmp_dir.path().join("bar.json");
        let another_input = "{\"list_children_batch_size\": 42}";
        File::create(&another_input_path).unwrap().write_all(another_input.as_bytes()).unwrap();

        let args = Args {
            output: output_path.clone(),
            input: vec![input_path, another_input_path],
            product: vec![],
        };
        compile(args)?;

        let mut bytes = Vec::new();
        File::open(output_path)?.read_to_end(&mut bytes)?;
        let config: component_internal::Config = unpersist(&bytes)?;
        assert_eq!(config.debug, Some(true));
        assert_eq!(config.list_children_batch_size, Some(42));
        Ok(())
    }

    #[test]
    fn test_deep_merge() -> Result<(), Error> {
        let tmp_dir = TempDir::new().unwrap();
        let output_path = tmp_dir.path().join("config");

        let input_path = tmp_dir.path().join("foo.json");
        let input = r#"{
            security_policy: {
                job_policy: {
                    ambient_mark_vmo_exec: ["/foo1"],
                    create_raw_processes: ["/foo1"],
                },
            },
        }"#;
        File::create(&input_path).unwrap().write_all(input.as_bytes()).unwrap();

        let another_input_path = tmp_dir.path().join("bar.json");
        let another_input = r#"{
            security_policy: {
                job_policy: {
                    ambient_mark_vmo_exec: ["/foo2"],
                    create_raw_processes: ["/foo2"],
                },
            },
        }"#;
        File::create(&another_input_path).unwrap().write_all(another_input.as_bytes()).unwrap();

        let args = Args {
            output: output_path.clone(),
            input: vec![input_path, another_input_path],
            product: vec![],
        };
        compile(args)?;

        let mut bytes = Vec::new();
        File::open(output_path)?.read_to_end(&mut bytes)?;
        let config: component_internal::Config = unpersist(&bytes)?;
        assert_eq!(
            config.security_policy,
            Some(component_internal::SecurityPolicy {
                job_policy: Some(component_internal::JobPolicyAllowlists {
                    ambient_mark_vmo_exec: Some(vec!["/foo1".to_string(), "/foo2".to_string()]),
                    create_raw_processes: Some(vec!["/foo1".to_string(), "/foo2".to_string()]),
                    ..Default::default()
                }),
                ..Default::default()
            })
        );
        Ok(())
    }

    #[test]
    fn test_invalid_component_url() {
        let input = r#"{
            root_component_url: "not quite a valid Url",
        }"#;
        assert_matches!(compile_str(input), Err(Error::Parse { .. }));
    }

    #[test]
    fn test_capability_allowlist_merging() {
        let left = vec![
            CapabilityAllowlistEntry {
                source_moniker: Some("moniker1".into()),
                source_name: Some("name1".into()),
                source: Some(CapabilityFrom::Capability),
                capability: Some(CapabilityTypeName::Protocol),
                target_monikers: Some(vec!["target1".into()]),
            },
            CapabilityAllowlistEntry {
                source_moniker: Some("moniker2".into()),
                source_name: Some("name2".into()),
                source: Some(CapabilityFrom::Capability),
                capability: Some(CapabilityTypeName::Protocol),
                target_monikers: Some(vec!["target2".into()]),
            },
        ];

        let right = vec![
            CapabilityAllowlistEntry {
                source_moniker: Some("moniker2".into()),
                source_name: Some("name2".into()),
                source: Some(CapabilityFrom::Capability),
                capability: Some(CapabilityTypeName::Protocol),
                target_monikers: Some(vec!["target3".into()]),
            },
            CapabilityAllowlistEntry {
                source_moniker: Some("moniker1".into()),
                source_name: Some("name1".into()),
                source: Some(CapabilityFrom::Capability),
                capability: Some(CapabilityTypeName::Protocol),
                target_monikers: Some(vec!["target4".into()]),
            },
        ];

        let expected_combine = vec![
            CapabilityAllowlistEntry {
                source_moniker: Some("moniker1".into()),
                source_name: Some("name1".into()),
                source: Some(CapabilityFrom::Capability),
                capability: Some(CapabilityTypeName::Protocol),
                target_monikers: Some(vec!["target1".into(), "target4".into()]),
            },
            CapabilityAllowlistEntry {
                source_moniker: Some("moniker2".into()),
                source_name: Some("name2".into()),
                source: Some(CapabilityFrom::Capability),
                capability: Some(CapabilityTypeName::Protocol),
                target_monikers: Some(vec!["target2".into(), "target3".into()]),
            },
        ];

        let combined = CapabilityAllowlistEntry::merge_vecs(left, right).unwrap();
        assert_eq!(combined, expected_combine);
    }

    #[test]
    fn test_capability_allowlist_merging_no_dups() {
        let left = vec![
            CapabilityAllowlistEntry {
                source_moniker: Some("moniker1".into()),
                source_name: Some("name1".into()),
                source: Some(CapabilityFrom::Capability),
                capability: Some(CapabilityTypeName::Protocol),
                target_monikers: Some(vec!["target1".into()]),
            },
            CapabilityAllowlistEntry {
                source_moniker: Some("moniker2".into()),
                source_name: Some("name2".into()),
                source: Some(CapabilityFrom::Capability),
                capability: Some(CapabilityTypeName::Protocol),
                target_monikers: Some(vec!["target2".into()]),
            },
        ];

        let right = left.clone();

        let expected_combine = left.clone();

        let combined = CapabilityAllowlistEntry::merge_vecs(left, right).unwrap();
        assert_eq!(combined, expected_combine);
    }

    #[test]
    fn test_product_and_platform() -> Result<(), Error> {
        let tmp_dir = TempDir::new().unwrap();
        let output_path = tmp_dir.path().join("config");

        let input_path = tmp_dir.path().join("foo.json");
        let input = r#"{
            security_policy: {
                job_policy: {
                    ambient_mark_vmo_exec: ["/foo1"],
                    create_raw_processes: ["/foo1"],
                },
                capability_policy: [
                    {
                        source_moniker: "<component_manager>",
                        source: "component",
                        source_name: "fuchsia.kernel.MmioResource",
                        capability: "protocol",
                        target_monikers: ["/root", "/root/bootstrap", "/root/core"],
                    },
                ],
            },
        }"#;
        File::create(&input_path).unwrap().write_all(input.as_bytes()).unwrap();

        let another_input_path = tmp_dir.path().join("bar.json");
        let another_input = r#"{
            ambient_mark_vmo_exec: {
                session: ["foo2"],
            },
            platform_capabilities: {
                session: [
                    {
                        capability: "fuchsia.lowpan.device.DeviceExtraConnector",
                        target_monikers: ["session_comp1"],
                    },
                ],
            },
            product_capabilities: {
                session: [
                    {
                        source_moniker: "session_comp1",
                        capability: "product.some.Resource",
                        target_monikers: ["session_comp2", "session_comp3"],
                    },
                ],
            },
        }"#;
        File::create(&another_input_path).unwrap().write_all(another_input.as_bytes()).unwrap();

        let args = Args {
            output: output_path.clone(),
            input: vec![input_path],
            product: vec![another_input_path],
        };
        compile(args)?;

        let mut bytes = Vec::new();
        File::open(output_path)?.read_to_end(&mut bytes)?;
        let config: component_internal::Config = unpersist(&bytes)?;
        assert_eq!(
            config.security_policy,
            Some(component_internal::SecurityPolicy {
                job_policy: Some(component_internal::JobPolicyAllowlists {
                    ambient_mark_vmo_exec: Some(vec![
                        "/core/session-manager/session:session/foo2".to_string(),
                        "/foo1".to_string()
                    ]),
                    create_raw_processes: Some(vec!["/foo1".to_string()]),
                    ..Default::default()
                }),
                capability_policy: Some(component_internal::CapabilityPolicyAllowlists {
                    allowlist: Some(vec![
                        component_internal::CapabilityAllowlistEntry {
                            source_moniker: Some("/core/lowpanservice".to_string()),
                            source_name: Some(
                                "fuchsia.lowpan.device.DeviceExtraConnector".to_string()
                            ),
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                            capability: Some(component_internal::AllowlistedCapability::Protocol(
                                component_internal::AllowlistedProtocol::default()
                            )),
                            target_monikers: Some(vec![
                                "/core/session-manager/session:session/session_comp1".to_string(),
                            ]),
                            ..Default::default()
                        },
                        component_internal::CapabilityAllowlistEntry {
                            source_moniker: Some(
                                "/core/session-manager/session:session/session_comp1".to_string()
                            ),
                            source_name: Some("product.some.Resource".to_string()),
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                            capability: Some(component_internal::AllowlistedCapability::Protocol(
                                component_internal::AllowlistedProtocol::default()
                            )),
                            target_monikers: Some(vec![
                                "/core/session-manager/session:session/session_comp2".to_string(),
                                "/core/session-manager/session:session/session_comp3".to_string(),
                            ]),
                            ..Default::default()
                        },
                        component_internal::CapabilityAllowlistEntry {
                            source_moniker: Some("<component_manager>".to_string()),
                            source_name: Some("fuchsia.kernel.MmioResource".to_string()),
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                            capability: Some(component_internal::AllowlistedCapability::Protocol(
                                component_internal::AllowlistedProtocol::default()
                            )),
                            target_monikers: Some(vec![
                                "/root".to_string(),
                                "/root/bootstrap".to_string(),
                                "/root/core".to_string()
                            ]),
                            ..Default::default()
                        },
                    ]),
                    ..Default::default()
                }),
                ..Default::default()
            })
        );
        Ok(())
    }

    #[test]
    fn test_product_only() -> Result<(), Error> {
        let tmp_dir = TempDir::new().unwrap();
        let output_path = tmp_dir.path().join("config");

        let another_input_path = tmp_dir.path().join("bar.json");
        let another_input = r#"{
            ambient_mark_vmo_exec: {
                session: ["foo2"],
            },
            platform_capabilities: {
                session: [
                    {
                        capability: "fuchsia.lowpan.device.DeviceExtraConnector",
                        target_monikers: ["session_comp1"],
                    },
                ],
            },
            product_capabilities: {
                session: [
                    {
                        source_moniker: "session_comp1",
                        capability: "product.some.Resource",
                        target_monikers: ["session_comp2", "session_comp3"],
                    },
                ],
            },
        }"#;
        File::create(&another_input_path).unwrap().write_all(another_input.as_bytes()).unwrap();

        let args =
            Args { output: output_path.clone(), input: vec![], product: vec![another_input_path] };
        compile(args)?;

        let mut bytes = Vec::new();
        File::open(output_path)?.read_to_end(&mut bytes)?;
        let config: component_internal::Config = unpersist(&bytes)?;
        assert_eq!(
            config.security_policy,
            Some(component_internal::SecurityPolicy {
                job_policy: Some(component_internal::JobPolicyAllowlists {
                    ambient_mark_vmo_exec: Some(vec![
                        "/core/session-manager/session:session/foo2".to_string(),
                    ]),
                    ..Default::default()
                }),
                capability_policy: Some(component_internal::CapabilityPolicyAllowlists {
                    allowlist: Some(vec![
                        component_internal::CapabilityAllowlistEntry {
                            source_moniker: Some(
                                "/core/session-manager/session:session/session_comp1".to_string()
                            ),
                            source_name: Some("product.some.Resource".to_string()),
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                            capability: Some(component_internal::AllowlistedCapability::Protocol(
                                component_internal::AllowlistedProtocol::default()
                            )),
                            target_monikers: Some(vec![
                                "/core/session-manager/session:session/session_comp2".to_string(),
                                "/core/session-manager/session:session/session_comp3".to_string(),
                            ]),
                            ..Default::default()
                        },
                        component_internal::CapabilityAllowlistEntry {
                            source_moniker: Some("/core/lowpanservice".to_string()),
                            source_name: Some(
                                "fuchsia.lowpan.device.DeviceExtraConnector".to_string()
                            ),
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                            capability: Some(component_internal::AllowlistedCapability::Protocol(
                                component_internal::AllowlistedProtocol::default()
                            )),
                            target_monikers: Some(vec![
                                "/core/session-manager/session:session/session_comp1".to_string(),
                            ]),
                            ..Default::default()
                        },
                    ]),
                    ..Default::default()
                }),
                ..Default::default()
            })
        );
        Ok(())
    }

    #[test]
    fn test_product_invalid_platform_capability() {
        let tmp_dir = TempDir::new().unwrap();
        let output_path = tmp_dir.path().join("config");

        let another_input_path = tmp_dir.path().join("bar.json");
        let another_input = r#"{
            platform_capabilities: {
                session: [
                    {
                        capability: "fuchsia.nonexistent.Resource",
                        target_monikers: ["session_comp1"],
                    },
                ],
            },
        }"#;
        File::create(&another_input_path).unwrap().write_all(another_input.as_bytes()).unwrap();

        let args =
            Args { output: output_path.clone(), input: vec![], product: vec![another_input_path] };
        assert_matches!(compile(args), Err(Error::Parse { .. }));
    }
}
