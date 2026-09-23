// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::one_or_many::OneOrMany;
use crate::types::common::*;
use crate::{
    AnyRef, AsClauseContext, ContextPathClause, Error, FromClauseContext, OfferFromRef,
    merge_spanned_vec,
};
pub use cm_types::{
    Availability, BorrowedName, BoundedName, DeliveryType, DependencyType, HandleType, Name,
    OnTerminate, ParseError, Path, RelativePath, StartupMode, StorageId, Url,
};
use cml_macro::Reference;
use reference_doc::ReferenceDoc;
use serde::{Deserialize, Serialize, de};

use std::fmt;
use std::sync::Arc;

/// Example:
///
/// ```json5
/// environments: [
///     {
///         name: "test-env",
///         extends: "realm",
///         runners: [
///             {
///                 runner: "gtest-runner",
///                 from: "#gtest",
///             },
///         ],
///         resolvers: [
///             {
///                 resolver: "full-resolver",
///                 from: "parent",
///                 scheme: "fuchsia-pkg",
///             },
///         ],
///     },
/// ],
/// ```
#[derive(Deserialize, Debug, PartialEq, ReferenceDoc, Serialize)]
#[serde(deny_unknown_fields)]
#[reference_doc(fields_as = "list", top_level_doc_after_fields)]
pub struct Environment {
    /// The name of the environment, which is a string of one or more of the
    /// following characters: `a-z`, `0-9`, `_`, `.`, `-`. The name identifies this
    /// environment when used in a [reference](#references).
    pub name: Name,

    /// How the environment should extend this realm's environment.
    /// - `realm`: Inherit all properties from this component's environment.
    /// - `none`: Start with an empty environment, do not inherit anything.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extends: Option<EnvironmentExtends>,

    /// The runners registered in the environment. An array of objects
    /// with the following properties:
    #[reference_doc(recurse)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runners: Option<Vec<RunnerRegistration>>,

    /// The resolvers registered in the environment. An array of
    /// objects with the following properties:
    #[reference_doc(recurse)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolvers: Option<Vec<ResolverRegistration>>,

    /// Debug protocols available to any component in this environment acquired
    /// through `use from debug`.
    #[reference_doc(recurse)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug: Option<Vec<DebugRegistration>>,

    /// The number of milliseconds to wait, after notifying a component in this environment that it
    /// should terminate, before forcibly killing it. This field is required if the environment
    /// extends from `none`.
    #[serde(rename = "__stop_timeout_ms")]
    #[reference_doc(json_type = "number", rename = "__stop_timeout_ms")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_timeout_ms: Option<StopTimeoutMs>,
}

/// A reference in an environment.
#[derive(Debug, PartialEq, Eq, Hash, Clone, Reference)]
#[reference(expected = "\"#<environment-name>\"")]
pub enum EnvironmentRef {
    /// A reference to an environment defined in this component.
    Named(Name),
}

#[derive(Deserialize, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EnvironmentExtends {
    Realm,
    None,
}

/// The stop timeout configured in an environment.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct StopTimeoutMs(pub u32);

impl<'de> de::Deserialize<'de> for StopTimeoutMs {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = StopTimeoutMs;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an unsigned 32-bit integer")
            }

            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if v < 0 || v > i64::from(u32::MAX) {
                    return Err(E::invalid_value(
                        de::Unexpected::Signed(v),
                        &"an unsigned 32-bit integer",
                    ));
                }
                Ok(StopTimeoutMs(v as u32))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                self.visit_i64(value as i64)
            }
        }

        deserializer.deserialize_i64(Visitor)
    }
}

/// A reference in an environment registration.
#[derive(Debug, PartialEq, Eq, Hash, Clone, Reference)]
#[reference(expected = "\"parent\", \"self\", or \"#<child-name>\"")]
pub enum RegistrationRef {
    /// A reference to a child.
    Named(Name),
    /// A reference to the parent.
    Parent,
    /// A reference to this component.
    Self_,
}

#[derive(Deserialize, Debug, PartialEq, ReferenceDoc, Serialize)]
#[serde(deny_unknown_fields)]
#[reference_doc(fields_as = "list")]
pub struct RunnerRegistration {
    /// The [name](#name) of a runner capability, whose source is specified in `from`.
    pub runner: Name,

    /// The source of the runner capability, one of:
    /// - `parent`: The component's parent.
    /// - `self`: This component.
    /// - `#<child-name>`: A [reference](#references) to a child component
    ///     instance.
    pub from: RegistrationRef,

    /// An explicit name for the runner as it will be known in
    /// this environment. If omitted, defaults to `runner`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#as: Option<Name>,
}

impl Hydrate for RunnerRegistration {
    type Output = ContextRunnerRegistration;

    fn hydrate(self, file: &Arc<std::path::Path>) -> Result<Self::Output, Error> {
        let runner = hydrate_simple(self.runner, file);

        let r#as = hydrate_opt_simple(self.r#as, file);

        let from = hydrate_simple(self.from, file);

        Ok(ContextRunnerRegistration { runner, r#as, from })
    }
}

#[derive(Debug, PartialEq, Serialize)]
pub struct ContextRunnerRegistration {
    pub runner: ContextSpanned<Name>,
    pub from: ContextSpanned<RegistrationRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#as: Option<ContextSpanned<Name>>,
}

impl FromClauseContext for ContextRunnerRegistration {
    fn from_(&self) -> ContextSpanned<OneOrMany<AnyRef<'_>>> {
        let origin = self.from.origin.clone();
        let value = OneOrMany::One(AnyRef::from(&self.from.value));

        ContextSpanned { value, origin }
    }
}

#[derive(Deserialize, Debug, PartialEq, ReferenceDoc, Serialize)]
#[serde(deny_unknown_fields)]
#[reference_doc(fields_as = "list")]
pub struct ResolverRegistration {
    /// The [name](#name) of a resolver capability,
    /// whose source is specified in `from`.
    pub resolver: Name,

    /// The source of the resolver capability, one of:
    /// - `parent`: The component's parent.
    /// - `self`: This component.
    /// - `#<child-name>`: A [reference](#references) to a child component
    ///     instance.
    pub from: RegistrationRef,

    /// The URL scheme for which the resolver should handle
    /// resolution.
    pub scheme: cm_types::UrlScheme,
}

impl Hydrate for ResolverRegistration {
    type Output = ContextResolverRegistration;

    fn hydrate(self, file: &Arc<std::path::Path>) -> Result<Self::Output, Error> {
        let resolver = hydrate_simple(self.resolver, file);

        let from = hydrate_simple(self.from, file);
        let scheme = hydrate_simple(self.scheme, file);

        Ok(ContextResolverRegistration { resolver, from, scheme })
    }
}

#[derive(Debug, PartialEq, Serialize)]
pub struct ContextResolverRegistration {
    pub resolver: ContextSpanned<Name>,
    pub from: ContextSpanned<RegistrationRef>,
    pub scheme: ContextSpanned<cm_types::UrlScheme>,
}

impl FromClauseContext for ContextResolverRegistration {
    fn from_(&self) -> ContextSpanned<OneOrMany<AnyRef<'_>>> {
        let origin = self.from.origin.clone();
        let value = OneOrMany::One(AnyRef::from(&self.from.value));

        ContextSpanned { value, origin }
    }
}

#[derive(Deserialize, Debug, Clone, PartialEq, ReferenceDoc, Serialize)]
#[serde(deny_unknown_fields)]
#[reference_doc(fields_as = "list")]
pub struct DebugRegistration {
    /// The name(s) of the protocol(s) to make available.
    pub protocol: Option<OneOrMany<Name>>,

    /// The source of the capability(s), one of:
    /// - `parent`: The component's parent.
    /// - `self`: This component.
    /// - `#<child-name>`: A [reference](#references) to a child component
    ///     instance.
    pub from: OfferFromRef,

    /// If specified, the name that the capability in `protocol` should be made
    /// available as to clients. Disallowed if `protocol` is an array.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#as: Option<Name>,
}

impl Hydrate for DebugRegistration {
    type Output = ContextDebugRegistration;

    fn hydrate(self, file: &Arc<std::path::Path>) -> Result<Self::Output, Error> {
        let origin = file.clone();
        let protocol = hydrate_opt_simple(self.protocol, file);
        let from = hydrate_simple(self.from, file);
        let r#as = hydrate_opt_simple(self.r#as, file);

        Ok(ContextDebugRegistration { origin, protocol, from, r#as })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContextDebugRegistration {
    #[serde(skip)]
    pub origin: Arc<std::path::Path>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<ContextSpanned<OneOrMany<Name>>>,
    pub from: ContextSpanned<OfferFromRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#as: Option<ContextSpanned<Name>>,
}

impl FromClauseContext for ContextDebugRegistration {
    fn from_(&self) -> ContextSpanned<OneOrMany<AnyRef<'_>>> {
        let origin = self.from.origin.clone();
        let value = OneOrMany::One(AnyRef::from(&self.from.value));

        ContextSpanned { value, origin }
    }
}

impl AsClauseContext for ContextDebugRegistration {
    fn r#as(&self) -> Option<ContextSpanned<&BorrowedName>> {
        self.r#as.as_ref().map(|spanned_name| ContextSpanned {
            value: spanned_name.value.as_ref(),
            origin: spanned_name.origin.clone(),
        })
    }
}

impl ContextPathClause for ContextDebugRegistration {
    fn path(&self) -> Option<&ContextSpanned<Path>> {
        None
    }
}

impl ContextCapabilityClause for ContextDebugRegistration {
    fn service(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>> {
        None
    }
    fn protocol(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>> {
        option_one_or_many_as_ref_context(&self.protocol)
    }
    fn directory(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>> {
        None
    }
    fn storage(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>> {
        None
    }
    fn runner(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>> {
        None
    }
    fn resolver(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>> {
        None
    }
    fn event_stream(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>> {
        None
    }
    fn dictionary(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>> {
        None
    }
    fn config(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>> {
        None
    }

    fn decl_type(&self) -> &'static str {
        "debug"
    }
    fn supported(&self) -> &[&'static str] {
        &["service", "protocol"]
    }
    fn are_many_names_allowed(&self) -> bool {
        ["protocol"].contains(&self.capability_type(None).unwrap())
    }

    fn set_service(&mut self, _o: Option<ContextSpanned<OneOrMany<Name>>>) {}
    fn set_protocol(&mut self, o: Option<ContextSpanned<OneOrMany<Name>>>) {
        self.protocol = o;
    }
    fn set_directory(&mut self, _o: Option<ContextSpanned<OneOrMany<Name>>>) {}
    fn set_storage(&mut self, _o: Option<ContextSpanned<OneOrMany<Name>>>) {}
    fn set_runner(&mut self, _o: Option<ContextSpanned<OneOrMany<Name>>>) {}
    fn set_resolver(&mut self, _o: Option<ContextSpanned<OneOrMany<Name>>>) {}
    fn set_event_stream(&mut self, _o: Option<ContextSpanned<OneOrMany<Name>>>) {}
    fn set_dictionary(&mut self, _o: Option<ContextSpanned<OneOrMany<Name>>>) {}
    fn set_config(&mut self, _o: Option<ContextSpanned<OneOrMany<Name>>>) {}

    fn origin(&self) -> &Arc<std::path::Path> {
        &self.origin
    }

    fn availability(&self) -> Option<ContextSpanned<Availability>> {
        None
    }
    fn set_availability(&mut self, _a: Option<ContextSpanned<Availability>>) {}
}

impl Hydrate for Environment {
    type Output = ContextEnvironment;

    fn hydrate(self, file: &Arc<std::path::Path>) -> Result<Self::Output, Error> {
        let name = hydrate_simple(self.name, file);

        let extends = hydrate_opt_simple(self.extends, file);
        let stop_timeout_ms = hydrate_opt_simple(self.stop_timeout_ms, file);

        let runners = hydrate_list(self.runners, file)?;
        let resolvers = hydrate_list(self.resolvers, file)?;
        let debug = hydrate_list(self.debug, file)?;

        Ok(ContextEnvironment { name, extends, runners, resolvers, debug, stop_timeout_ms })
    }
}

#[derive(Debug, PartialEq, Serialize)]
pub struct ContextEnvironment {
    pub name: ContextSpanned<Name>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extends: Option<ContextSpanned<EnvironmentExtends>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runners: Option<Vec<ContextSpanned<ContextRunnerRegistration>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolvers: Option<Vec<ContextSpanned<ContextResolverRegistration>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug: Option<Vec<ContextSpanned<ContextDebugRegistration>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "__stop_timeout_ms")]
    pub stop_timeout_ms: Option<ContextSpanned<StopTimeoutMs>>,
}

impl ContextEnvironment {
    pub fn merge_from(&mut self, mut other: Self) -> Result<(), Error> {
        if let Some(other_extends) = other.extends.take() {
            if let Some(my_extends) = &self.extends {
                if my_extends.value != other_extends.value {
                    return Err(Error::merge(
                        format!(
                            "Conflicting 'extends' field in environment '{}': found '{:?}' and '{:?}'",
                            self.name.value, my_extends.value, other_extends.value
                        ),
                        Some(other_extends.origin),
                    ));
                }
            } else {
                self.extends = Some(other_extends);
            }
        }

        if let Some(other_timeout) = other.stop_timeout_ms.take() {
            if let Some(my_timeout) = &self.stop_timeout_ms {
                if my_timeout.value != other_timeout.value {
                    return Err(Error::merge(
                        format!(
                            "Conflicting 'stop_timeout_ms' in environment '{}'",
                            self.name.value
                        ),
                        Some(other_timeout.origin),
                    ));
                }
            } else {
                self.stop_timeout_ms = Some(other_timeout);
            }
        }

        merge_spanned_vec!(self, other, runners);
        merge_spanned_vec!(self, other, resolvers);
        merge_spanned_vec!(self, other, debug);

        Ok(())
    }
}
