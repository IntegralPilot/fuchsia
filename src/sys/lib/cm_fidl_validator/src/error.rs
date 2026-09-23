// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use cm_types::ParseError;
use fidl_fuchsia_component_decl as fdecl;
use std::fmt;
use std::fmt::Display;
use thiserror::Error;

/// Enum type that can represent any error encountered during validation.
#[derive(Debug, Error, PartialEq, Clone)]
pub enum Error {
    #[error("Field `{}` is missing for {}.", .0.field, .0.decl)]
    MissingField(DeclField),

    #[error("Field `{}` is empty for {}.", .0.field, .0.decl)]
    EmptyField(DeclField),

    #[error("{} has unnecessary field `{}`.", .0.decl, .0.field)]
    ExtraneousField(DeclField),

    #[error("\"{}\" is duplicated for field `{}` in {}.", .1, .0.field, .0.decl)]
    DuplicateField(DeclField, String),

    #[error("Field `{}` for {} is invalid.",  .0.field, .0.decl)]
    InvalidField(DeclField),

    #[error("Field {} for {} is invalid. {}.", .0.field, .0.decl, .1)]
    InvalidUrl(DeclField, String),

    #[error("Field `{}` for {} is too long.", .0.field, .0.decl)]
    FieldTooLong(DeclField),

    #[error("Field `{}` for {} has an invalid path segment.", .0.field, .0.decl)]
    FieldInvalidSegment(DeclField),

    #[error("\"{0}\" capabilities must be offered as a built-in capability.")]
    CapabilityMustBeBuiltin(DeclType),

    #[error("\"{0}\" capabilities are not currently allowed as built-ins.")]
    CapabilityCannotBeBuiltin(DeclType),

    #[error(
        "Encountered an unknown capability declaration. This may happen due to ABI skew between the FIDL component declaration and the system."
    )]
    UnknownCapability,

    #[error("\"{1}\" is referenced in {0} but it does not appear in children.")]
    InvalidChild(DeclField, String),

    #[error("\"{1}\" is referenced in {0} but it does not appear in collections.")]
    InvalidCollection(DeclField, String),

    #[error("\"{1}\" is referenced in {0} but it does not appear in storage.")]
    InvalidStorage(DeclField, String),

    #[error("\"{1}\" is referenced in {0} but it does not appear in environments.")]
    InvalidEnvironment(DeclField, String),

    #[error("\"{1}\" is referenced in {0} but it does not appear in capabilities.")]
    InvalidCapability(DeclField, String),

    #[error("\"{1}\" is referenced in {0} but it does not appear in runners.")]
    InvalidRunner(DeclField, String),

    #[error("There are dependency cycle(s): {0}.")]
    DependencyCycle(String),

    #[error(
        "Path \"{path}\" from {decl} overlaps with \"{other_path}\" path from {other_decl}. Paths across decls must be unique in order to avoid namespace collisions."
    )]
    InvalidPathOverlap { decl: DeclField, path: String, other_decl: DeclField, other_path: String },

    #[error("{} \"{}\" path overlaps with \"/pkg\", which is a protected path", decl, path)]
    PkgPathOverlap { decl: DeclField, path: String },

    #[error(
        "Source path \"{1}\" provided to {0} decl is unnecessary. Built-in capabilities don't need this field as they originate from the framework."
    )]
    ExtraneousSourcePath(DeclField, String),

    #[error(
        "Configuration schema defines a vector nested inside another vector. Vector can only contain numbers, booleans, and strings."
    )]
    NestedVector,

    #[error(
        "The `availability` field in {0} for {1} must be set to \"optional\" because the source is \"void\"."
    )]
    AvailabilityMustBeOptional(DeclField, String),

    #[error("Invalid aggregate offer: {0}")]
    InvalidAggregateOffer(String),

    #[error(
        "All sources that feed into an aggregation operation should have the same availability. Got {0}."
    )]
    DifferentAvailabilityInAggregation(AvailabilityList),

    #[error("Multiple runners used.")]
    MultipleRunnersUsed,

    #[error("Used runner conflicts with program runner.")]
    ConflictingRunners,

    #[error(
        "Runner is missing for executable component. A runner must be specified in the \
            `program` section or in the `use` section."
    )]
    MissingRunner,

    #[error("Dynamic children cannot specify an environment.")]
    DynamicChildWithEnvironment,
}

/// [AvailabilityList] is a newtype to provide a human friendly [Display] impl for a vector
/// of availabilities.
#[derive(Debug, PartialEq, Clone)]
pub struct AvailabilityList(pub Vec<fdecl::Availability>);

impl Display for AvailabilityList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let comma_separated =
            self.0.iter().map(|s| format!("{:?}", s)).collect::<Vec<_>>().join(", ");
        write!(f, "[ {comma_separated} ]")
    }
}

impl Error {
    pub fn missing_field(decl_type: DeclType, keyword: impl Into<String>) -> Self {
        Error::MissingField(DeclField { decl: decl_type, field: keyword.into() })
    }

    pub fn empty_field(decl_type: DeclType, keyword: impl Into<String>) -> Self {
        Error::EmptyField(DeclField { decl: decl_type, field: keyword.into() })
    }

    pub fn extraneous_field(decl_type: DeclType, keyword: impl Into<String>) -> Self {
        Error::ExtraneousField(DeclField { decl: decl_type, field: keyword.into() })
    }

    pub fn duplicate_field(
        decl_type: DeclType,
        keyword: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        Error::DuplicateField(DeclField { decl: decl_type, field: keyword.into() }, value.into())
    }

    pub fn invalid_field(decl_type: DeclType, keyword: impl Into<String>) -> Self {
        Error::InvalidField(DeclField { decl: decl_type, field: keyword.into() })
    }

    pub fn invalid_url(
        decl_type: DeclType,
        keyword: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Error::InvalidUrl(DeclField { decl: decl_type, field: keyword.into() }, message.into())
    }

    pub fn field_too_long(decl_type: DeclType, keyword: impl Into<String>) -> Self {
        Error::FieldTooLong(DeclField { decl: decl_type, field: keyword.into() })
    }

    pub fn field_invalid_segment(decl_type: DeclType, keyword: impl Into<String>) -> Self {
        Error::FieldInvalidSegment(DeclField { decl: decl_type, field: keyword.into() })
    }

    pub fn invalid_child(
        decl_type: DeclType,
        keyword: impl Into<String>,
        child: impl Into<String>,
    ) -> Self {
        Error::InvalidChild(DeclField { decl: decl_type, field: keyword.into() }, child.into())
    }

    pub fn invalid_collection(
        decl_type: DeclType,
        keyword: impl Into<String>,
        collection: impl Into<String>,
    ) -> Self {
        Error::InvalidCollection(
            DeclField { decl: decl_type, field: keyword.into() },
            collection.into(),
        )
    }

    pub fn invalid_environment(
        decl_type: DeclType,
        keyword: impl Into<String>,
        environment: impl Into<String>,
    ) -> Self {
        Error::InvalidEnvironment(
            DeclField { decl: decl_type, field: keyword.into() },
            environment.into(),
        )
    }

    // TODO: Replace with `invalid_capability`?
    pub fn invalid_runner(
        decl_type: DeclType,
        keyword: impl Into<String>,
        runner: impl Into<String>,
    ) -> Self {
        Error::InvalidRunner(DeclField { decl: decl_type, field: keyword.into() }, runner.into())
    }

    pub fn invalid_capability(
        decl_type: DeclType,
        keyword: impl Into<String>,
        capability: impl Into<String>,
    ) -> Self {
        Error::InvalidCapability(
            DeclField { decl: decl_type, field: keyword.into() },
            capability.into(),
        )
    }

    pub fn dependency_cycle(error: impl Into<String>) -> Self {
        Error::DependencyCycle(error.into())
    }

    pub fn invalid_path_overlap(
        decl: DeclType,
        path: impl Into<String>,
        other_decl: DeclType,
        other_path: impl Into<String>,
    ) -> Self {
        Error::InvalidPathOverlap {
            decl: DeclField { decl, field: "target_path".to_string() },
            path: path.into(),
            other_decl: DeclField { decl: other_decl, field: "target_path".to_string() },
            other_path: other_path.into(),
        }
    }

    pub fn pkg_path_overlap(decl: DeclType, path: impl Into<String>) -> Self {
        Error::PkgPathOverlap {
            decl: DeclField { decl, field: "target_path".to_string() },
            path: path.into(),
        }
    }

    pub fn extraneous_source_path(decl_type: DeclType, path: impl Into<String>) -> Self {
        Error::ExtraneousSourcePath(
            DeclField { decl: decl_type, field: "source_path".to_string() },
            path.into(),
        )
    }

    pub fn nested_vector() -> Self {
        Error::NestedVector
    }

    pub fn availability_must_be_optional(
        decl_type: DeclType,
        keyword: impl Into<String>,
        source_name: Option<&String>,
    ) -> Self {
        Error::AvailabilityMustBeOptional(
            DeclField { decl: decl_type, field: keyword.into() },
            source_name.cloned().unwrap_or_else(|| "<unnamed>".to_string()),
        )
    }

    pub fn invalid_aggregate_offer(info: impl Into<String>) -> Self {
        Error::InvalidAggregateOffer(info.into())
    }

    pub fn different_availability_in_aggregation(availability: Vec<fdecl::Availability>) -> Self {
        Error::DifferentAvailabilityInAggregation(AvailabilityList(availability))
    }

    pub fn from_parse_error(
        err: ParseError,
        prop: &String,
        decl_type: DeclType,
        keyword: &str,
    ) -> Self {
        match err {
            ParseError::Empty => Error::empty_field(decl_type, keyword),
            ParseError::TooLong => Error::field_too_long(decl_type, keyword),
            ParseError::InvalidComponentUrl { details } => {
                Error::invalid_url(decl_type, keyword, format!(r#""{prop}": {details}"#))
            }
            ParseError::InvalidValue => Error::invalid_field(decl_type, keyword),
            ParseError::InvalidSegment => Error::field_invalid_segment(decl_type, keyword),
            ParseError::NoLeadingSlash => Error::invalid_field(decl_type, keyword),
        }
    }
}

// To regenerate:
//
// ```
//     fx exec env | \
//         grep FUCHSIA_BUILD_DIR | \
//         xargs -I {} bash -c 'export {}; grep -E "pub (enum|struct)" $FUCHSIA_BUILD_DIR/fidling/gen/sdk/fidl/fuchsia.component.decl/fuchsia.component.decl/rust/fidl_fuchsia_component_decl.rs' | \
//         awk '{print $3}' | \
//         sed 's/[:;]$//' | \
//         sort | uniq | sed 's/$/,/'
// ```
//
/// The list of all declarations in fuchsia.component.decl, for error reporting purposes.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum DeclType {
    AllowedOffers,
    Availability,
    Capability,
    CapabilityRef,
    Child,
    ChildRef,
    Collection,
    CollectionRef,
    Component,
    Configuration,
    ConfigChecksum,
    ConfigField,
    ConfigMutability,
    ConfigOverride,
    ConfigSchema,
    ConfigSingleValue,
    ConfigType,
    ConfigTypeLayout,
    ConfigValue,
    ConfigValuesData,
    ConfigValueSource,
    ConfigValueSpec,
    ConfigVectorValue,
    DebugProtocolRegistration,
    DebugRef,
    DebugRegistration,
    DependencyType,
    Dictionary,
    Directory,
    Durability,
    Environment,
    EnvironmentExtends,
    EventStream,
    EventSubscription,
    Expose,
    ExposeConfig,
    ExposeDictionary,
    ExposeDirectory,
    ExposeProtocol,
    ExposeResolver,
    ExposeRunner,
    ExposeService,
    FrameworkRef,
    LayoutConstraint,
    LayoutParameter,
    NameMapping,
    Offer,
    OfferConfig,
    OfferDictionary,
    OfferDirectory,
    OfferEventStream,
    OfferProtocol,
    OfferResolver,
    OfferRunner,
    OfferService,
    OfferStorage,
    OnTerminate,
    ParentRef,
    Program,
    Protocol,
    Ref,
    ResolvedConfig,
    ResolvedConfigField,
    Resolver,
    ResolverRegistration,
    Runner,
    RunnerRegistration,
    SelfRef,
    Service,
    StartupMode,
    Storage,
    StorageId,
    Use,
    UseConfiguration,
    UseDictionary,
    UseDirectory,
    UseEventStream,
    UseProtocol,
    UseRunner,
    UseService,
    UseStorage,
    VoidRef,
}

impl fmt::Display for DeclType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match *self {
            // To regenerate:
            //
            // ```
            //     fx exec env | \
            //         grep FUCHSIA_BUILD_DIR | \
            //         xargs -I {} bash -c 'export {}; grep -E "pub (enum|struct)" $FUCHSIA_BUILD_DIR/fidling/gen/sdk/fidl/fuchsia.component.decl/fuchsia.component.decl/rust/fidl_fuchsia_component_decl.rs' | \
            //         awk '{print $3}' | \
            //         sed 's/[:;]$//' | \
            //         sort | uniq | sed 's/\(.*\)/DeclType::\1 => "\1",/'
            // ```
            DeclType::AllowedOffers => "AllowedOffers",
            DeclType::Availability => "Availability",
            DeclType::Capability => "Capability",
            DeclType::CapabilityRef => "CapabilityRef",
            DeclType::Child => "Child",
            DeclType::ChildRef => "ChildRef",
            DeclType::Collection => "Collection",
            DeclType::CollectionRef => "CollectionRef",
            DeclType::Component => "Component",
            DeclType::Configuration => "Configuration",
            DeclType::ConfigChecksum => "ConfigChecksum",
            DeclType::ConfigField => "ConfigField",
            DeclType::ConfigMutability => "ConfigMutability",
            DeclType::ConfigOverride => "ConfigOverride",
            DeclType::ConfigSchema => "ConfigSchema",
            DeclType::ConfigSingleValue => "ConfigSingleValue",
            DeclType::ConfigType => "ConfigType",
            DeclType::ConfigTypeLayout => "ConfigTypeLayout",
            DeclType::ConfigValue => "ConfigValue",
            DeclType::ConfigValuesData => "ConfigValuesData",
            DeclType::ConfigValueSource => "ConfigValueSource",
            DeclType::ConfigValueSpec => "ConfigValueSpec",
            DeclType::ConfigVectorValue => "ConfigVectorValue",
            DeclType::DebugProtocolRegistration => "DebugProtocolRegistration",
            DeclType::DebugRef => "DebugRef",
            DeclType::DebugRegistration => "DebugRegistration",
            DeclType::DependencyType => "DependencyType",
            DeclType::Dictionary => "Dictionary",
            DeclType::Directory => "Directory",
            DeclType::Durability => "Durability",
            DeclType::Environment => "Environment",
            DeclType::EnvironmentExtends => "EnvironmentExtends",
            DeclType::EventStream => "EventStream",
            DeclType::EventSubscription => "EventSubscription",
            DeclType::Expose => "Expose",
            DeclType::ExposeConfig => "ExposeConfig",
            DeclType::ExposeDictionary => "ExposeDictionary",
            DeclType::ExposeDirectory => "ExposeDirectory",
            DeclType::ExposeProtocol => "ExposeProtocol",
            DeclType::ExposeResolver => "ExposeResolver",
            DeclType::ExposeRunner => "ExposeRunner",
            DeclType::ExposeService => "ExposeService",
            DeclType::FrameworkRef => "FrameworkRef",
            DeclType::LayoutConstraint => "LayoutConstraint",
            DeclType::LayoutParameter => "LayoutParameter",
            DeclType::NameMapping => "NameMapping",
            DeclType::Offer => "Offer",
            DeclType::OfferConfig => "OfferConfig",
            DeclType::OfferDictionary => "OfferDictionary",
            DeclType::OfferDirectory => "OfferDirectory",
            DeclType::OfferEventStream => "OfferEventStream",
            DeclType::OfferProtocol => "OfferProtocol",
            DeclType::OfferResolver => "OfferResolver",
            DeclType::OfferRunner => "OfferRunner",
            DeclType::OfferService => "OfferService",
            DeclType::OfferStorage => "OfferStorage",
            DeclType::OnTerminate => "OnTerminate",
            DeclType::ParentRef => "ParentRef",
            DeclType::Program => "Program",
            DeclType::Protocol => "Protocol",
            DeclType::Ref => "Ref",
            DeclType::ResolvedConfig => "ResolvedConfig",
            DeclType::ResolvedConfigField => "ResolvedConfigField",
            DeclType::Resolver => "Resolver",
            DeclType::ResolverRegistration => "ResolverRegistration",
            DeclType::Runner => "Runner",
            DeclType::RunnerRegistration => "RunnerRegistration",
            DeclType::SelfRef => "SelfRef",
            DeclType::Service => "Service",
            DeclType::StartupMode => "StartupMode",
            DeclType::Storage => "Storage",
            DeclType::StorageId => "StorageId",
            DeclType::Use => "Use",
            DeclType::UseConfiguration => "UseConfiguration",
            DeclType::UseDictionary => "UseDictionary",
            DeclType::UseDirectory => "UseDirectory",
            DeclType::UseEventStream => "UseEventStream",
            DeclType::UseProtocol => "UseProtocol",
            DeclType::UseRunner => "UseRunner",
            DeclType::UseService => "UseService",
            DeclType::UseStorage => "UseStorage",
            DeclType::VoidRef => "VoidRef",
        };
        write!(f, "{}", name)
    }
}

impl From<&fdecl::Use> for DeclType {
    fn from(use_: &fdecl::Use) -> Self {
        match use_ {
            fdecl::Use::Config(_) => DeclType::UseConfiguration,
            #[cfg(fuchsia_api_level_at_least = "30")]
            fdecl::Use::Dictionary(_) => DeclType::UseDictionary,
            fdecl::Use::Directory(_) => DeclType::UseDirectory,
            fdecl::Use::EventStream(_) => DeclType::UseEventStream,
            fdecl::Use::Protocol(_) => DeclType::UseProtocol,
            #[cfg(fuchsia_api_level_at_least = "HEAD")]
            fdecl::Use::Runner(_) => DeclType::UseRunner,
            fdecl::Use::Service(_) => DeclType::UseService,
            fdecl::Use::Storage(_) => DeclType::UseStorage,
            _ => DeclType::Use,
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct DeclField {
    pub decl: DeclType,
    pub field: String,
}

impl fmt::Display for DeclField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.decl, self.field)
    }
}

/// Represents a list of errors encountered during validation.
#[derive(Debug, Error, PartialEq, Clone)]
pub struct ErrorList {
    pub errs: Vec<Error>,
}

impl ErrorList {
    pub(crate) fn new(errs: Vec<Error>) -> ErrorList {
        ErrorList { errs }
    }
}

impl fmt::Display for ErrorList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let strs: Vec<String> = self.errs.iter().map(|e| format!("{}", e)).collect();
        write!(f, "{}", strs.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_errors() {
        assert_eq!(
            format!("{}", Error::missing_field(DeclType::Child, "keyword")),
            "Field `keyword` is missing for Child."
        );
        assert_eq!(
            format!("{}", Error::empty_field(DeclType::Child, "keyword")),
            "Field `keyword` is empty for Child."
        );
        assert_eq!(
            format!("{}", Error::duplicate_field(DeclType::Child, "keyword", "foo")),
            "\"foo\" is duplicated for field `keyword` in Child."
        );
        assert_eq!(
            format!("{}", Error::invalid_field(DeclType::Child, "keyword")),
            "Field `keyword` for Child is invalid."
        );
        assert_eq!(
            format!("{}", Error::field_too_long(DeclType::Child, "keyword")),
            "Field `keyword` for Child is too long."
        );
        assert_eq!(
            format!("{}", Error::field_invalid_segment(DeclType::Child, "keyword")),
            "Field `keyword` for Child has an invalid path segment."
        );
        assert_eq!(
            format!("{}", Error::invalid_child(DeclType::Child, "source", "child")),
            "\"child\" is referenced in Child.source but it does not appear in children."
        );
        assert_eq!(
            format!("{}", Error::invalid_collection(DeclType::Child, "source", "child")),
            "\"child\" is referenced in Child.source but it does not appear in collections."
        );
    }
}
