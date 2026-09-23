// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::realm::{get_resolved_declaration, resolve_declaration};
use cm_rust::NativeIntoFidl;
use config_value_file::field::config_value_from_json_value;
use flex_fuchsia_component_decl as fdecl;
use flex_fuchsia_sys2 as fsys;
use moniker::Moniker;

pub async fn resolve_raw_config_overrides(
    realm_query: &fsys::RealmQueryProxy,
    moniker: &Moniker,
    url: &str,
    raw_overrides: &[RawConfigEntry],
) -> Result<Vec<fdecl::ConfigOverride>, ConfigResolveError> {
    // Don't worry about any of the below failure modes if there aren't actually any overrides.
    if raw_overrides.is_empty() {
        return Ok(vec![]);
    }

    let manifest = resolve_manifest(moniker, realm_query, url).await?;
    let config = manifest.config.as_ref().ok_or(ConfigResolveError::MissingConfigSchema)?;

    let mut resolved_overrides = vec![];
    for raw_override in raw_overrides {
        let config_field =
            config.fields.iter().find(|f| f.key == raw_override.key).ok_or_else(|| {
                ConfigResolveError::MissingConfigField { name: raw_override.key.clone() }
            })?;

        let resolved_value = config_value_from_json_value(&raw_override.value, &config_field.type_)
            .map_err(ConfigResolveError::FieldTypeError)?;
        resolved_overrides.push(fdecl::ConfigOverride {
            key: Some(raw_override.key.clone()),
            value: Some(resolved_value.native_into_fidl()),
            ..Default::default()
        });
    }

    Ok(resolved_overrides)
}

pub async fn resolve_raw_config_capabilities(
    realm_query: &fsys::RealmQueryProxy,
    moniker: &Moniker,
    url: &str,
    raw_capabilities: &[RawConfigEntry],
) -> Result<Vec<fdecl::Configuration>, ConfigResolveError> {
    // Don't worry about any of the below failure modes if there aren't actually any overrides.
    if raw_capabilities.is_empty() {
        return Ok(vec![]);
    }

    let manifest = resolve_manifest(moniker, realm_query, url).await?;
    let config_uses: Vec<_> = IntoIterator::into_iter(manifest.uses)
        .filter_map(|u| if let cm_rust::UseDecl::Config(c) = u { Some(c) } else { None })
        .collect();

    let mut resolved_capabilities = vec![];
    for raw_capability in raw_capabilities {
        let config_field =
            config_uses.iter().find(|f| f.source_name == raw_capability.key.as_str()).ok_or_else(
                || ConfigResolveError::MissingConfigField { name: raw_capability.key.clone() },
            )?;

        let resolved_value =
            config_value_from_json_value(&raw_capability.value, &config_field.type_)
                .map_err(ConfigResolveError::FieldTypeError)?;
        resolved_capabilities.push(fdecl::Configuration {
            name: Some(raw_capability.key.clone()),
            value: Some(resolved_value.native_into_fidl()),
            ..Default::default()
        });
    }

    Ok(resolved_capabilities)
}

pub(crate) enum UseConfigurationOrConfigField {
    UseConfiguration(cm_rust::UseConfigurationDecl),
    ConfigField(cm_rust::ConfigField),
}

pub(crate) async fn resolve_config_decls(
    realm_query: &fsys::RealmQueryProxy,
    moniker: &Moniker,
) -> Result<Vec<UseConfigurationOrConfigField>, ConfigResolveError> {
    let manifest = get_resolved_declaration(moniker, realm_query).await?;
    let config_decls = manifest
        .config
        .into_iter()
        .map(|c| c.fields)
        .flatten()
        .map(UseConfigurationOrConfigField::ConfigField);
    Ok(IntoIterator::into_iter(manifest.uses)
        .filter_map(|u| if let cm_rust::UseDecl::Config(c) = u { Some(*c) } else { None })
        .map(UseConfigurationOrConfigField::UseConfiguration)
        .chain(config_decls)
        .collect())
}

async fn resolve_manifest(
    moniker: &Moniker,
    realm_query: &fsys::RealmQueryProxy,
    url: &str,
) -> Result<cm_rust::ComponentDecl, ConfigResolveError> {
    let parent = moniker.parent().ok_or_else(|| ConfigResolveError::BadMoniker(moniker.clone()))?;
    let leaf = moniker.leaf().ok_or_else(|| ConfigResolveError::BadMoniker(moniker.clone()))?;
    let collection =
        leaf.collection().ok_or_else(|| ConfigResolveError::BadMoniker(moniker.clone()))?;
    let child_location = fsys::ChildLocation::Collection(collection.to_string());
    let manifest = resolve_declaration(realm_query, &parent, &child_location, url).await?;
    Ok(manifest)
}

/// [`RawConfigEntry`] may either represent a config override (where the key is
/// usually `snake_case`) or a configuration capability declaration (where the
/// key is usually `fully.qualified.Name`).
#[derive(Debug, PartialEq)]
pub struct RawConfigEntry {
    key: String,
    value: serde_json::Value,
}

impl std::str::FromStr for RawConfigEntry {
    type Err = ConfigParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (key, value) = s.split_once("=").ok_or(ConfigParseError::MissingEqualsSeparator)?;
        Ok(Self {
            key: key.to_owned(),
            value: serde_json::from_str(&value).map_err(ConfigParseError::InvalidJsonValue)?,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigResolveError {
    #[error("`{_0}` does not reference a dynamic instance.")]
    BadMoniker(Moniker),

    #[error("Failed to get component manifest: {_0:?}")]
    FailedToGetManifest(
        #[source]
        #[from]
        crate::realm::GetDeclarationError,
    ),

    #[error("Provided component URL points to a manifest without a config schema.")]
    MissingConfigSchema,

    #[error("Component does not have a config field named `{name}`.")]
    MissingConfigField { name: String },

    #[error("Couldn't resolve provided config value to the declared type in component manifest.")]
    FieldTypeError(#[source] config_value_file::field::FieldError),
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigParseError {
    #[error("Config override did not have a `=` to delimit key and value strings.")]
    MissingEqualsSeparator,

    #[error("Unable to parse provided value as JSON.")]
    InvalidJsonValue(#[source] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use std::str::FromStr;

    #[test]
    fn parse_config() {
        let config = RawConfigEntry::from_str("foo=\"bar\"").unwrap();
        assert_eq!(config.key, "foo");
        assert_matches!(config.value, serde_json::Value::String(s) if &s == "bar");

        let config = RawConfigEntry::from_str("foo=true").unwrap();
        assert_eq!(config.key, "foo");
        assert_matches!(config.value, serde_json::Value::Bool(true));

        RawConfigEntry::from_str("invalid").unwrap_err();
    }

    #[test]
    fn parse_config_capabilities() {
        let config = RawConfigEntry::from_str("fuchsia.my.Config=\"bar\"").unwrap();
        assert_eq!(config.key, "fuchsia.my.Config");
        assert_matches!(config.value, serde_json::Value::String(s) if &s == "bar");
    }
}
