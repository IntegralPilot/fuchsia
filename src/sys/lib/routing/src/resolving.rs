// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::component_instance::{ComponentInstanceInterface, ExtendedInstanceInterface};
use crate::error::ComponentInstanceError;
use anyhow::Error;
use clonable_error::ClonableError;
use cm_graph::DependencyNode;
use directed_graph::DirectedGraph;
use fidl_fuchsia_component_resolution as fresolution;
use fidl_fuchsia_io as fio;
use std::sync::{Arc, LazyLock};
use thiserror::Error;
use url::Url;
use version_history::AbiRevision;
use zx_status as zx;

#[cfg(target_os = "fuchsia")]
use cm_rust::{FidlIntoNative, NativeIntoFidl};

/// The prefix for relative URLs internally represented as url::Url.
const RELATIVE_URL_PREFIX: &str = "relative:///";
/// A default base URL from which to parse relative component URL
/// components.
static RELATIVE_URL_BASE: LazyLock<Url> =
    LazyLock::new(|| Url::parse(RELATIVE_URL_PREFIX).unwrap());

/// The response returned from a Resolver. This struct is derived from the FIDL
/// [`fuchsia.component.resolution.Component`][fidl_fuchsia_component_resolution::Component]
/// table, except that the opaque binary ComponentDecl has been deserialized and validated.
#[derive(Debug)]
pub struct ResolvedComponent {
    /// The package context, from the component resolution context returned by
    /// the resolver.
    pub context_to_resolve_children: Option<ComponentResolutionContext>,
    pub decl: cm_rust::ComponentDecl,
    pub package: Option<ResolvedPackage>,
    pub config_values: Option<cm_rust::ConfigValuesData>,
    pub abi_revision: Option<AbiRevision>,
    pub dependencies: DirectedGraph<DependencyNode>,
}

// This block and others in this file only build on target, because these functions rely on
// mem_util which has a test dependency on `vfs`, which isn't typically allowed to be built on
// host.
#[cfg(target_os = "fuchsia")]
impl TryFrom<fresolution::Component> for ResolvedComponent {
    type Error = ResolverError;

    fn try_from(component: fresolution::Component) -> Result<Self, Self::Error> {
        let decl_buffer: fidl_fuchsia_mem::Data =
            component.decl.ok_or(ResolverError::RemoteInvalidData)?;
        let mut dependencies = DirectedGraph::new();
        let decl = read_and_validate_manifest(&decl_buffer, &mut dependencies)?;
        let config_values = match &decl.config {
            Some(config) => match config.value_source {
                cm_rust::ConfigValueSource::PackagePath(_) => {
                    Some(read_and_validate_config_values(
                        &component.config_values.ok_or(ResolverError::RemoteInvalidData)?,
                    )?)
                }
                cm_rust::ConfigValueSource::Capabilities(_) => None,
            },
            None => None,
        };
        let context_to_resolve_children = component.resolution_context.map(Into::into);
        let abi_revision = component.abi_revision.map(Into::into);
        Ok(ResolvedComponent {
            context_to_resolve_children,
            decl,
            package: component.package.map(TryInto::try_into).transpose()?,
            config_values,
            abi_revision,
            dependencies,
        })
    }
}

#[cfg(target_os = "fuchsia")]
impl From<ResolvedComponent> for fresolution::Component {
    fn from(component: ResolvedComponent) -> Self {
        let ResolvedComponent {
            context_to_resolve_children,
            decl,
            package,
            config_values,
            abi_revision,
            dependencies: _,
        } = component;
        let decl_bytes = fidl::persist(&decl.native_into_fidl())
            .expect("failed to serialize validated manifest");
        let decl_vmo = fidl::Vmo::create(decl_bytes.len() as u64).expect("failed to create VMO");
        decl_vmo.write(&decl_bytes, 0).expect("failed to write to VMO");
        fresolution::Component {
            url: None,
            decl: Some(fidl_fuchsia_mem::Data::Buffer(fidl_fuchsia_mem::Buffer {
                vmo: decl_vmo,
                size: decl_bytes.len() as u64,
            })),
            package: package.map(|p| fresolution::Package {
                url: Some(p.url),
                directory: Some(p.directory),
                ..Default::default()
            }),
            config_values: config_values.map(|config_values| {
                let config_values_bytes = fidl::persist(&config_values.native_into_fidl())
                    .expect("failed to serialize config values");
                let config_values_vmo = fidl::Vmo::create(config_values_bytes.len() as u64)
                    .expect("failed to create VMO");
                config_values_vmo.write(&config_values_bytes, 0).expect("failed to write to VMO");
                fidl_fuchsia_mem::Data::Buffer(fidl_fuchsia_mem::Buffer {
                    vmo: config_values_vmo,
                    size: config_values_bytes.len() as u64,
                })
            }),
            resolution_context: context_to_resolve_children.map(Into::into),
            abi_revision: abi_revision.map(Into::into),
            ..Default::default()
        }
    }
}

#[cfg(target_os = "fuchsia")]
pub fn read_and_validate_manifest(
    data: &fidl_fuchsia_mem::Data,
    dependencies: &mut DirectedGraph<DependencyNode>,
) -> Result<cm_rust::ComponentDecl, ResolverError> {
    let bytes = mem_util::bytes_from_data(data).map_err(ResolverError::manifest_invalid)?;
    read_and_validate_manifest_bytes(&bytes, dependencies)
}

#[cfg(target_os = "fuchsia")]
pub fn read_and_validate_manifest_bytes(
    bytes: &[u8],
    dependencies: &mut DirectedGraph<DependencyNode>,
) -> Result<cm_rust::ComponentDecl, ResolverError> {
    let component_decl: fidl_fuchsia_component_decl::Component =
        fidl::unpersist(bytes).map_err(ResolverError::manifest_invalid)?;
    cm_fidl_validator::validate(&component_decl, dependencies)
        .map_err(ResolverError::manifest_invalid)?;
    Ok(component_decl.fidl_into_native())
}

#[cfg(target_os = "fuchsia")]
pub fn read_and_validate_config_values(
    data: &fidl_fuchsia_mem::Data,
) -> Result<cm_rust::ConfigValuesData, ResolverError> {
    let bytes = mem_util::bytes_from_data(&data).map_err(ResolverError::config_values_invalid)?;
    let values = fidl::unpersist(&bytes).map_err(ResolverError::fidl_error)?;
    cm_fidl_validator::validate_values_data(&values)
        .map_err(|e| ResolverError::config_values_invalid(e))?;
    Ok(values.fidl_into_native())
}

/// The response returned from a Resolver. This struct is derived from the FIDL
/// [`fuchsia.component.resolution.Package`][fidl_fuchsia_component_resolution::Package]
/// table.
#[derive(Debug)]
pub struct ResolvedPackage {
    /// The package url.
    pub url: String,
    /// The package directory client proxy.
    pub directory: fidl::endpoints::ClientEnd<fio::DirectoryMarker>,
}

impl TryFrom<fresolution::Package> for ResolvedPackage {
    type Error = ResolverError;

    fn try_from(package: fresolution::Package) -> Result<Self, Self::Error> {
        Ok(ResolvedPackage {
            url: package.url.ok_or(ResolverError::PackageUrlMissing)?,
            directory: package.directory.ok_or(ResolverError::PackageDirectoryMissing)?,
        })
    }
}

/// Convenience wrapper type for the autogenerated FIDL
/// `fuchsia.component.resolution.Context`.
#[derive(Debug, Clone, PartialEq, Eq, Ord, PartialOrd, Hash)]
pub struct ComponentResolutionContext {
    pub bytes: Vec<u8>,
}

impl ComponentResolutionContext {
    pub fn new(bytes: Vec<u8>) -> Self {
        ComponentResolutionContext { bytes }
    }
}

impl From<fresolution::Context> for ComponentResolutionContext {
    fn from(context: fresolution::Context) -> Self {
        ComponentResolutionContext { bytes: context.bytes }
    }
}

impl From<&fresolution::Context> for ComponentResolutionContext {
    fn from(context: &fresolution::Context) -> ComponentResolutionContext {
        ComponentResolutionContext { bytes: context.bytes.clone() }
    }
}

impl From<ComponentResolutionContext> for fresolution::Context {
    fn from(context: ComponentResolutionContext) -> Self {
        Self { bytes: context.bytes }
    }
}

impl From<&ComponentResolutionContext> for fresolution::Context {
    fn from(context: &ComponentResolutionContext) -> fresolution::Context {
        Self { bytes: context.bytes.clone() }
    }
}

impl<'a> From<&'a ComponentResolutionContext> for &'a [u8] {
    fn from(context: &'a ComponentResolutionContext) -> &'a [u8] {
        &context.bytes
    }
}

/// Provides the `ComponentAddress` and context for resolving a child or
/// descendent component.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedAncestorComponent {
    /// The component address, needed for relative path URLs (to get the
    /// scheme used to find the required `Resolver`), or for relative resource
    /// URLs (which will clone the parent's address, but replace the resource).
    pub address: ComponentAddress,
    /// The component's resolution_context, required for resolving descendents
    /// using a relative path component URLs.
    pub context_to_resolve_children: Option<ComponentResolutionContext>,
}

impl ResolvedAncestorComponent {
    /// Creates a `ResolvedAncestorComponent` from one of its child components.
    pub async fn direct_parent_of<C: ComponentInstanceInterface>(
        component: &Arc<C>,
    ) -> Result<Self, ResolverError> {
        let parent_component = get_parent(component).await?;
        let resolved_parent = parent_component.lock_resolved_state().await?;
        Ok(Self {
            address: resolved_parent.address().await?,
            context_to_resolve_children: resolved_parent.context_to_resolve_children(),
        })
    }

    /// Creates a `ResolvedAncestorComponent` from one of its child components.
    pub async fn first_packaged_ancestor_of<C: ComponentInstanceInterface>(
        component: &Arc<C>,
    ) -> Result<Self, ResolverError> {
        let mut parent_component = get_parent(component).await?;
        loop {
            // Loop until the parent has a valid context_to_resolve_children,
            // or an error getting the next parent, or its resolved state.
            {
                let resolved_parent = parent_component.lock_resolved_state().await?;
                let address = resolved_parent.address().await?;
                // TODO(https://fxbug.dev/42053123): change this test to something more
                // explicit, that is, return the parent's address and context if
                // the component address is a packaged component (determined in
                // some way).
                //
                // The issue being addressed here is, when resolving a relative
                // subpackaged component URL, component manager MUST resolve the
                // component using a "resolution context" _AND_ the resolver
                // that provided that context. Typically these are provided by
                // the parent component, but in the case of a RealmBuilder child
                // component, its parent is the built "realm" (which was
                // resolved by the realm_builder_resolver, using URL scheme
                // "realm-builder://"). The child component's URL is supposed to
                // be relative to the test component (the parent of the realm),
                // which was probably resolved by the full-resolver (scheme
                // "fuchsia-pkg://"). Knowing this expected topology, we can
                // skip "realm-builder" components when searching for the
                // required ancestor's URL scheme (to get the right resolver)
                // and context. This is a brittle workaround that will be
                // replaced.
                //
                // Some alternatives are under discussion, but the leading
                // candidate, for now, is to allow a resolver to return a flag
                // (with the resolved Component; perhaps `is_packaged()`) to
                // indicate that descendents should (if true) use this component
                // to get scheme and context for resolving relative path URLs
                // (for example, subpackages). If false, get the parent's parent
                // and so on.
                if address.scheme() != "realm-builder" {
                    return Ok(Self {
                        address,
                        context_to_resolve_children: resolved_parent.context_to_resolve_children(),
                    });
                }
            }
            parent_component = get_parent(&parent_component).await?;
        }
    }
}

async fn get_parent<C: ComponentInstanceInterface>(
    component: &Arc<C>,
) -> Result<Arc<C>, ResolverError> {
    if let ExtendedInstanceInterface::Component(parent_component) =
        component.try_get_parent().map_err(|err| {
            ResolverError::no_parent_context(anyhow::format_err!(
                "Component {} ({}) has no parent for context: {:?}.",
                component.moniker(),
                component.url(),
                err,
            ))
        })?
    {
        Ok(parent_component)
    } else {
        Err(ResolverError::no_parent_context(anyhow::format_err!(
            "Component {} ({}) has no parent for context.",
            component.moniker(),
            component.url(),
        )))
    }
}

/// Indicates the kind of `ComponentAddress`, and holds `ComponentAddress` properties specific to
/// its kind. Note that there is no kind for a relative resource component URL (a URL that only
/// contains a resource fragment, such as `#meta/comp.cm`) because `ComponentAddress::from_url()`
/// and `ComponentAddress::from_url_and_context()` will translate a resource fragment component URL
/// into one of the fully-resolvable `ComponentAddress`s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComponentAddress {
    /// A fully-qualified component URL.
    Absolute { url: Url },

    /// A relative Component URL, starting with the package path; for example a
    /// subpackage relative URL such as "needed_package#meta/dep_component.cm".
    RelativePath {
        /// This is the scheme of the ancestor component's absolute URL, used to identify the
        /// `Resolver` in a `ResolverRegistry`.
        scheme: String,

        /// The relative URL, represented as a `url::Url` with the `relative:///` base prefix.
        /// `url::Url` cannot represent relative urls directly.
        url: Url,

        /// An opaque value (from the perspective of component resolution)
        /// required by the resolver when resolving a relative package path.
        /// For a given child component, this property is populated from a
        /// parent component's `resolution_context`, as returned by the parent
        /// component's resolver.
        context: ComponentResolutionContext,
    },
}

impl ComponentAddress {
    /// Creates a new `ComponentAddress` of the `Absolute` kind.
    fn new_absolute(url: Url) -> Self {
        Self::Absolute { url }
    }

    /// Creates a new `ComponentAddress` of the `RelativePath` kind.
    pub fn new_relative_path(
        path: &str,
        some_resource: Option<&str>,
        scheme: &str,
        context: ComponentResolutionContext,
    ) -> Result<Self, ResolverError> {
        let mut url = RELATIVE_URL_BASE.clone();
        url.set_path(path);
        url.set_fragment(some_resource);
        Self::check_relative_url(&url)?;
        Ok(Self::RelativePath { url, context, scheme: scheme.into() })
    }

    /// Parse the given absolute `component_url` and create a `ComponentAddress`
    /// with kind `Absolute`.
    pub fn from_absolute_url(component_url: &cm_types::Url) -> Result<Self, ResolverError> {
        match Url::parse(component_url.as_str()) {
            Ok(url) => Ok(Self::new_absolute(url)),
            Err(url::ParseError::RelativeUrlWithoutBase) => {
                Err(ResolverError::RelativeUrlNotExpected(component_url.to_string()))
            }
            Err(err) => Err(ResolverError::malformed_url(err)),
        }
    }

    /// Assumes the given string is a relative URL, and converts this string into
    /// a `url::Url` by attaching a known, internal absolute URL prefix.
    fn parse_relative_url(component_url: &cm_types::Url) -> Result<Url, ResolverError> {
        let component_url = component_url.as_str();
        match Url::parse(component_url) {
            Ok(_) => Err(ResolverError::malformed_url(anyhow::format_err!(
                "Error parsing a relative URL given absolute URL '{}'.",
                component_url,
            ))),
            Err(url::ParseError::RelativeUrlWithoutBase) => {
                RELATIVE_URL_BASE.join(component_url).map_err(|err| {
                    ResolverError::malformed_url(anyhow::format_err!(
                        "Error parsing a relative component URL '{}': {:?}.",
                        component_url,
                        err
                    ))
                })
            }
            Err(err) => Err(ResolverError::malformed_url(anyhow::format_err!(
                "Unexpected error while parsing a component URL '{}': {:?}.",
                component_url,
                err,
            ))),
        }
    }

    fn check_relative_url(url: &Url) -> Result<(), ResolverError> {
        let truncated_url = url.as_str().strip_prefix(RELATIVE_URL_PREFIX).ok_or_else(|| {
            ResolverError::malformed_url(anyhow::format_err!(
                "Could not strip relative prefix from url. This is a bug. {}",
                url
            ))
        })?;
        let relative_url = RELATIVE_URL_BASE.make_relative(&url).ok_or_else(|| {
            ResolverError::malformed_url(anyhow::format_err!(
                "Could not make relative url. This is a bug. {}",
                url
            ))
        })?;
        if truncated_url != relative_url {
            return Err(ResolverError::malformed_url(anyhow::format_err!(
                "Relative url generated from url::Url did not match expectations. \
                This is a bug. {}",
                url
            )));
        }
        Ok(())
    }

    /// For `Url`s constructed from `parse_relative_url()`, the `path()` is expected
    /// to include a slash prefix, but a String representation of a relative path
    /// URL should not include the slash prefix. This convenience API assumes the
    /// given `Url` is a relative URL, and strips the slash prefix, if present.
    fn relative_path(relative_url: &Url) -> &str {
        let path = relative_url.path();
        path.strip_prefix('/').unwrap_or(path)
    }

    /// Parse the given `component_url` to determine if it is an absolute URL, a relative
    /// subpackage URL, or a relative resource URL, and return the corresponding `ComponentAddress`
    /// enum variant and value. If the URL is relative, use the component instance to get the
    /// required resolution context from the component's parent.
    pub async fn from_url<C: ComponentInstanceInterface>(
        component_url: &cm_types::Url,
        component: &Arc<C>,
    ) -> Result<Self, ResolverError> {
        Self::from(component_url, None, component).await
    }

    /// Parse the given `component_url` to determine if it is an absolute URL, a relative
    /// subpackage URL, or a relative resource URL, and return the corresponding `ComponentAddress`
    /// enum variant and value. If the URL is relative, the provided context is used.
    pub async fn from_url_and_context<C: ComponentInstanceInterface>(
        component_url: &cm_types::Url,
        context: ComponentResolutionContext,
        component: &Arc<C>,
    ) -> Result<Self, ResolverError> {
        Self::from(component_url, Some(context), component).await
    }

    /// This function is the helper for `from_url` and `from_url_and_context`. It assembles a new
    /// `ComponentAddress`, using the provided resolution context. If a resolution context is not
    /// provided, and the URL is a relative URL, the component's parent will be used to create a
    /// context.
    pub async fn from<C: ComponentInstanceInterface>(
        component_url: &cm_types::Url,
        context: Option<ComponentResolutionContext>,
        component: &Arc<C>,
    ) -> Result<Self, ResolverError> {
        let result = Self::from_absolute_url(component_url);
        if !matches!(result, Err(ResolverError::RelativeUrlNotExpected(_))) {
            return result;
        }
        let relative_url = Self::parse_relative_url(component_url)?;
        let relative_path = Self::relative_path(&relative_url);
        if relative_url.fragment().is_none() && relative_path.is_empty() {
            return Err(ResolverError::malformed_url(anyhow::format_err!("{}", component_url)));
        }
        if relative_url.query().is_some() {
            return Err(ResolverError::malformed_url(anyhow::format_err!(
                "Query strings are not allowed in relative component URLs: {}",
                component_url
            )));
        }
        if relative_path.is_empty() {
            // The `component_url` had only a fragment, so the new address will be the same as its
            // parent (for example, the same package), except for its resource.
            let resolved_parent = ResolvedAncestorComponent::direct_parent_of(component).await?;
            resolved_parent.address.clone_with_new_resource(relative_url.fragment())
        } else {
            // The `component_url` starts with a relative path (for example, a subpackage name).
            // Create a `RelativePath` address, and resolve it using the
            // `context_to_resolve_children`, from this component's parent, or the first ancestor
            // that is from a "package". (Note that Realm Builder realms are synthesized, and not
            // from a package. A test component using Realm Builder will build a realm and may add
            // child components using subpackage references. Those child components should get
            // resolved using the context of the test package, not the intermediate realm created
            // via RealmBuilder.)
            let resolved_ancestor =
                ResolvedAncestorComponent::first_packaged_ancestor_of(component).await?;
            let scheme = resolved_ancestor.address.scheme();
            if let Some(context) = context {
                Self::new_relative_path(relative_path, relative_url.fragment(), scheme, context)
            } else {
                let context = resolved_ancestor.context_to_resolve_children.clone().ok_or_else(|| {
                        ResolverError::RelativeUrlMissingContext(format!(
                            "Relative path component URL '{}' cannot be resolved because its ancestor did not provide a resolution context. The ancestor's component address is {:?}.",
                             component_url, resolved_ancestor.address
                        ))
                    })?;
                Self::new_relative_path(relative_path, relative_url.fragment(), scheme, context)
            }
        }
    }

    /// Creates a new `ComponentAddress` from `self` by replacing only the
    /// component URL resource.
    pub fn clone_with_new_resource(
        &self,
        some_resource: Option<&str>,
    ) -> Result<Self, ResolverError> {
        self.clone().consume_with_new_resource(some_resource)
    }

    pub fn consume_with_new_resource(
        mut self,
        some_resource: Option<&str>,
    ) -> Result<Self, ResolverError> {
        let url = match &mut self {
            Self::Absolute { url } => url,
            Self::RelativePath { url, .. } => url,
        };
        url.set_fragment(some_resource);
        match self {
            Self::Absolute { url } => Ok(Self::Absolute { url }),
            Self::RelativePath { context, scheme, url } => {
                Self::check_relative_url(&url)?;
                Ok(Self::RelativePath { url, context, scheme })
            }
        }
    }

    /// True if the address is `Absolute`.
    pub fn is_absolute(&self) -> bool {
        matches!(self, Self::Absolute { .. })
    }

    /// True if the address is `RelativePath`.
    pub fn is_relative_path(&self) -> bool {
        matches!(self, Self::RelativePath { .. })
    }

    /// Returns the `ComponentResolutionContext` value required to resolve for a
    /// `RelativePath` component URL.
    ///
    /// Panics if called for an `Absolute` component address.
    pub fn context(&self) -> &ComponentResolutionContext {
        if let Self::RelativePath { context, .. } = self {
            &context
        } else {
            panic!("context() is only valid for `ComponentAddressKind::RelativePath");
        }
    }

    /// Returns the URL scheme either provided for an `Absolute` URL or derived
    /// from the component's parent. The scheme is used to look up a registered
    /// resolver, when resolving the component.
    pub fn scheme(&self) -> &str {
        match self {
            Self::Absolute { url } => url.scheme(),
            Self::RelativePath { scheme, .. } => &scheme,
        }
    }

    /// Returns the URL path.
    pub fn path(&self) -> &str {
        match self {
            Self::Absolute { url } => url.path(),
            Self::RelativePath { url, .. } => Self::relative_path(&url),
        }
    }

    /// Returns the optional query value for an `Absolute` component URL.
    /// Always returns `None` for `Relative` component URLs.
    pub fn query(&self) -> Option<&str> {
        match self {
            Self::Absolute { url } => url.query(),
            Self::RelativePath { .. } => None,
        }
    }

    /// Returns the optional component resource, from the URL fragment.
    pub fn resource(&self) -> Option<&str> {
        match self {
            Self::Absolute { url } => url.fragment(),
            Self::RelativePath { url, .. } => url.fragment(),
        }
    }

    /// Returns the resolver-ready URL string and, if it is a `RelativePath`,
    /// `Some(context)`, or `None` for an `Absolute` address.
    pub fn url(&self) -> &str {
        match self {
            Self::Absolute { url } => url.as_str(),
            Self::RelativePath { url, .. } => &url.as_str()[RELATIVE_URL_PREFIX.len()..],
        }
    }

    /// Returns the `url()` and `Some(context)` for resolving the URL,
    /// if the kind is `RelativePath` (or `None` if `Absolute`).
    pub fn to_url_and_context(&self) -> (&str, Option<&ComponentResolutionContext>) {
        match self {
            Self::Absolute { .. } => (self.url(), None),
            Self::RelativePath { context, .. } => (self.url(), Some(context)),
        }
    }
}

/// Errors produced by built-in `Resolver`s and `resolving` APIs.
#[derive(Debug, Error, Clone)]
pub enum ResolverError {
    #[error("an unexpected error occurred: {0}")]
    Internal(#[source] ClonableError),
    #[error("an IO error occurred: {0}")]
    Io(#[source] ClonableError),
    #[error("component manifest not found: {0}")]
    ManifestNotFound(#[source] ClonableError),
    #[error("package not found: {0}")]
    PackageNotFound(#[source] ClonableError),
    #[error("component manifest invalid: {0}")]
    ManifestInvalid(#[source] ClonableError),
    #[error("config values file invalid: {0}")]
    ConfigValuesInvalid(#[source] ClonableError),
    #[error("abi revision not found")]
    AbiRevisionNotFound,
    #[error("abi revision invalid: {0}")]
    AbiRevisionInvalid(#[source] ClonableError),
    #[error("failed to read config values: {0}")]
    ConfigValuesIo(zx::Status),
    #[error("scheme not registered")]
    SchemeNotRegistered,
    #[error("malformed url: {0}")]
    MalformedUrl(#[source] ClonableError),
    #[error("relative url requires a parent component with resolution context: {0}")]
    NoParentContext(#[source] ClonableError),
    #[error("package URL missing")]
    PackageUrlMissing,
    #[error("package directory handle missing")]
    PackageDirectoryMissing,
    #[error("a relative URL was not expected: {0}")]
    RelativeUrlNotExpected(String),
    #[error("failed to route resolver capability: {0}")]
    RoutingError(#[source] ClonableError),
    #[error("a context is required to resolve relative url: {0}")]
    RelativeUrlMissingContext(String),
    #[error("this component resolver does not resolve relative path component URLs: {0}")]
    UnexpectedRelativePath(String),
    #[error("the remote resolver returned invalid data")]
    RemoteInvalidData,
    #[error("an error occurred sending a FIDL request to the remote resolver: {0}")]
    FidlError(#[source] ClonableError),
}

impl ResolverError {
    pub fn as_zx_status(&self) -> zx::Status {
        match self {
            ResolverError::PackageNotFound(_)
            | ResolverError::ManifestNotFound(_)
            | ResolverError::ManifestInvalid(_)
            | ResolverError::ConfigValuesInvalid(_)
            | ResolverError::Io(_)
            | ResolverError::ConfigValuesIo(_)
            | ResolverError::AbiRevisionNotFound
            | ResolverError::AbiRevisionInvalid(_)
            | ResolverError::SchemeNotRegistered
            | ResolverError::MalformedUrl(_)
            | ResolverError::NoParentContext(_)
            | ResolverError::RelativeUrlMissingContext(_)
            | ResolverError::RemoteInvalidData
            | ResolverError::PackageUrlMissing
            | ResolverError::PackageDirectoryMissing
            | ResolverError::UnexpectedRelativePath(_) => zx::Status::NOT_FOUND,

            ResolverError::Internal(_)
            | ResolverError::RelativeUrlNotExpected(_)
            | ResolverError::RoutingError(_)
            | ResolverError::FidlError(_) => zx::Status::INTERNAL,
        }
    }

    pub fn internal(err: impl Into<Error>) -> Self {
        Self::Internal(err.into().into())
    }

    pub fn io(err: impl Into<Error>) -> Self {
        Self::Io(err.into().into())
    }

    pub fn manifest_not_found(err: impl Into<Error>) -> Self {
        Self::ManifestNotFound(err.into().into())
    }

    pub fn package_not_found(err: impl Into<Error>) -> Self {
        Self::PackageNotFound(err.into().into())
    }

    pub fn manifest_invalid(err: impl Into<Error>) -> Self {
        Self::ManifestInvalid(err.into().into())
    }

    pub fn config_values_invalid(err: impl Into<Error>) -> Self {
        Self::ConfigValuesInvalid(err.into().into())
    }

    pub fn abi_revision_invalid(err: impl Into<Error>) -> Self {
        Self::AbiRevisionInvalid(err.into().into())
    }

    pub fn malformed_url(err: impl Into<Error>) -> Self {
        Self::MalformedUrl(err.into().into())
    }

    pub fn no_parent_context(err: impl Into<Error>) -> Self {
        Self::NoParentContext(err.into().into())
    }

    pub fn routing_error(err: impl Into<Error>) -> Self {
        Self::RoutingError(err.into().into())
    }

    pub fn fidl_error(err: impl Into<Error>) -> Self {
        Self::FidlError(err.into().into())
    }
}

impl From<fresolution::ResolverError> for ResolverError {
    fn from(err: fresolution::ResolverError) -> ResolverError {
        match err {
            fresolution::ResolverError::Internal => ResolverError::internal(RemoteError(err)),
            fresolution::ResolverError::Io => ResolverError::io(RemoteError(err)),
            fresolution::ResolverError::PackageNotFound
            | fresolution::ResolverError::NoSpace
            | fresolution::ResolverError::ResourceUnavailable
            | fresolution::ResolverError::NotSupported => {
                ResolverError::package_not_found(RemoteError(err))
            }
            fresolution::ResolverError::ManifestNotFound => {
                ResolverError::manifest_not_found(RemoteError(err))
            }
            fresolution::ResolverError::InvalidArgs => {
                ResolverError::malformed_url(RemoteError(err))
            }
            fresolution::ResolverError::InvalidManifest => {
                ResolverError::ManifestInvalid(anyhow::Error::from(RemoteError(err)).into())
            }
            fresolution::ResolverError::ConfigValuesNotFound => {
                ResolverError::ConfigValuesIo(zx::Status::NOT_FOUND)
            }
            fresolution::ResolverError::AbiRevisionNotFound => ResolverError::AbiRevisionNotFound,
            fresolution::ResolverError::InvalidAbiRevision => {
                ResolverError::abi_revision_invalid(RemoteError(err))
            }
        }
    }
}

impl From<ResolverError> for fresolution::ResolverError {
    fn from(err: ResolverError) -> fresolution::ResolverError {
        match err {
            ResolverError::Internal(_) => fresolution::ResolverError::Internal,
            ResolverError::Io(_) => fresolution::ResolverError::Io,
            ResolverError::ManifestNotFound(_) => fresolution::ResolverError::ManifestNotFound,
            ResolverError::PackageNotFound(_) => fresolution::ResolverError::PackageNotFound,
            ResolverError::ManifestInvalid(_) => fresolution::ResolverError::InvalidManifest,
            ResolverError::ConfigValuesInvalid(_) => fresolution::ResolverError::InvalidManifest,
            ResolverError::AbiRevisionNotFound => fresolution::ResolverError::AbiRevisionNotFound,
            ResolverError::AbiRevisionInvalid(_) => fresolution::ResolverError::InvalidAbiRevision,
            ResolverError::ConfigValuesIo(_) => fresolution::ResolverError::Io,
            ResolverError::SchemeNotRegistered => fresolution::ResolverError::NotSupported,
            ResolverError::MalformedUrl(_) => fresolution::ResolverError::InvalidArgs,
            ResolverError::NoParentContext(_) => fresolution::ResolverError::Internal,
            ResolverError::PackageUrlMissing => fresolution::ResolverError::PackageNotFound,
            ResolverError::PackageDirectoryMissing => fresolution::ResolverError::PackageNotFound,
            ResolverError::RelativeUrlNotExpected(_) => fresolution::ResolverError::InvalidArgs,
            ResolverError::RoutingError(_) => fresolution::ResolverError::Internal,
            ResolverError::RelativeUrlMissingContext(_) => fresolution::ResolverError::InvalidArgs,
            ResolverError::UnexpectedRelativePath(_) => fresolution::ResolverError::InvalidArgs,
            ResolverError::RemoteInvalidData => fresolution::ResolverError::InvalidManifest,
            ResolverError::FidlError(_) => fresolution::ResolverError::Internal,
        }
    }
}

impl From<ComponentInstanceError> for ResolverError {
    fn from(err: ComponentInstanceError) -> ResolverError {
        use ComponentInstanceError::*;
        match &err {
            ComponentManagerInstanceUnavailable {}
            | ComponentManagerInstanceUnexpected {}
            | InstanceNotFound { .. }
            | InstanceNotExecutable { .. }
            | ResolveFailed { .. }
            | StartFailed { .. }
            | FailedToCreateStorage { .. } => {
                ResolverError::Internal(ClonableError::from(anyhow::format_err!("{:?}", err)))
            }
            NoAbsoluteUrl { .. } => ResolverError::NoParentContext(ClonableError::from(
                anyhow::format_err!("{:?}", err),
            )),
            MalformedUrl { .. } => {
                ResolverError::MalformedUrl(ClonableError::from(anyhow::format_err!("{:?}", err)))
            }
        }
    }
}

#[derive(Error, Clone, Debug)]
#[error("remote resolver responded with {0:?}")]
struct RemoteError(fresolution::ResolverError);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bedrock::sandbox_construction::ComponentSandbox;
    use crate::component_instance::{ResolvedInstanceInterface, TopInstanceInterface};
    use crate::policy::GlobalPolicyChecker;
    use assert_matches::assert_matches;
    use async_trait::async_trait;
    use capability_source::{BuiltinCapabilities, NamespaceCapabilities};
    use cm_rust::offer::OfferDecl;
    use cm_rust::{CapabilityDecl, CollectionDecl, ExposeDecl, UseDecl};
    use cm_rust_testing::new_decl_from_json;
    use cm_types::Name;
    use fidl::endpoints::create_endpoints;
    use fidl_fuchsia_component_decl as fdecl;
    use fidl_fuchsia_mem as fmem;
    use moniker::{BorrowedChildName, ChildName, Moniker};
    use serde_json::json;

    fn from_absolute_url(url: &str) -> ComponentAddress {
        ComponentAddress::from_absolute_url(&url.parse().unwrap()).unwrap()
    }

    fn parse_relative_url(url: &str) -> Url {
        ComponentAddress::parse_relative_url(&url.parse().unwrap()).unwrap()
    }

    #[test]
    fn test_resolved_package() {
        let url = "some_url".to_string();
        let (dir_client, _) = create_endpoints::<fio::DirectoryMarker>();
        let fidl_package = fresolution::Package {
            url: Some(url.clone()),
            directory: Some(dir_client),
            ..Default::default()
        };
        let resolved_package = ResolvedPackage::try_from(fidl_package).unwrap();
        assert_eq!(resolved_package.url, url);
    }

    #[test]
    fn test_component_address() {
        let address = from_absolute_url("some-scheme://fuchsia.com/package#meta/comp.cm");
        assert!(address.is_absolute());
        assert_eq!(address.scheme(), "some-scheme");
        assert_eq!(address.path(), "/package");
        assert_eq!(address.query(), None);
        assert_eq!(address.resource(), Some("meta/comp.cm"));
        assert_eq!(address.url(), "some-scheme://fuchsia.com/package#meta/comp.cm");
        assert_matches!(
            address.to_url_and_context(),
            ("some-scheme://fuchsia.com/package#meta/comp.cm", None)
        );

        let abs_address = ComponentAddress::new_absolute(
            Url::parse("some-scheme://fuchsia.com/package#meta/comp.cm").unwrap(),
        );
        assert_eq!(abs_address, address);

        assert_eq!(abs_address, address);
        assert!(abs_address.is_absolute());
        assert_eq!(abs_address.scheme(), "some-scheme");
        assert_eq!(abs_address.path(), "/package");
        assert_eq!(abs_address.query(), None);
        assert_eq!(abs_address.resource(), Some("meta/comp.cm"));
        assert_eq!(abs_address.url(), "some-scheme://fuchsia.com/package#meta/comp.cm");
        assert_matches!(
            abs_address.to_url_and_context(),
            ("some-scheme://fuchsia.com/package#meta/comp.cm", None)
        );

        let cloned_address = abs_address.clone();
        assert_eq!(abs_address, cloned_address);

        let address2 = abs_address.clone_with_new_resource(Some("meta/other_comp.cm")).unwrap();
        assert_ne!(address2, abs_address);
        assert!(address2.is_absolute());
        assert_eq!(address2.resource(), Some("meta/other_comp.cm"));
        assert_eq!(address2.scheme(), "some-scheme");
        assert_eq!(address2.path(), "/package");
        assert_eq!(address2.query(), None);

        let rel_address = ComponentAddress::new_relative_path(
            "subpackage",
            Some("meta/subcomp.cm"),
            "some-scheme",
            ComponentResolutionContext::new(vec![b'4', b'5', b'6']),
        )
        .unwrap();
        if let ComponentAddress::RelativePath { ref context, .. } = rel_address {
            assert_eq!(&context.bytes, &vec![b'4', b'5', b'6']);
        }
        assert!(rel_address.is_relative_path());
        assert_eq!(rel_address.path(), "subpackage");
        assert_eq!(rel_address.query(), None);
        assert_eq!(rel_address.resource(), Some("meta/subcomp.cm"));
        assert_eq!(&rel_address.context().bytes, &vec![b'4', b'5', b'6']);
        assert_eq!(rel_address.url(), "subpackage#meta/subcomp.cm");
        assert_eq!(
            rel_address.to_url_and_context(),
            (
                "subpackage#meta/subcomp.cm",
                Some(&ComponentResolutionContext::new(vec![b'4', b'5', b'6']))
            )
        );

        let rel_address2 =
            rel_address.clone_with_new_resource(Some("meta/other_subcomp.cm")).unwrap();
        assert_ne!(rel_address2, rel_address);
        assert!(rel_address2.is_relative_path());
        assert_eq!(rel_address2.path(), "subpackage");
        assert_eq!(rel_address2.query(), None);
        assert_eq!(rel_address2.resource(), Some("meta/other_subcomp.cm"));
        assert_eq!(&rel_address2.context().bytes, &vec![b'4', b'5', b'6']);
        assert_eq!(rel_address2.url(), "subpackage#meta/other_subcomp.cm");
        assert_eq!(
            rel_address2.to_url_and_context(),
            (
                "subpackage#meta/other_subcomp.cm",
                Some(&ComponentResolutionContext::new(vec![b'4', b'5', b'6']))
            )
        );

        let address = from_absolute_url("base://b");
        assert!(address.is_absolute());
        assert_eq!(address.scheme(), "base");
        assert_eq!(address.path(), "");
        assert_eq!(address.query(), None);
        assert_eq!(address.resource(), None);
        assert_eq!(address.url(), "base://b");
        assert_matches!(address.to_url_and_context(), ("base://b", None));

        let address = from_absolute_url("fuchsia-boot:///#meta/root.cm");
        assert!(address.is_absolute());
        assert_eq!(address.scheme(), "fuchsia-boot");
        assert_eq!(address.path(), "/");
        assert_eq!(address.query(), None);
        assert_eq!(address.resource(), Some("meta/root.cm"));
        assert_eq!(address.url(), "fuchsia-boot:///#meta/root.cm");
        assert_matches!(address.to_url_and_context(), ("fuchsia-boot:///#meta/root.cm", None));

        let address = from_absolute_url("custom-resolver:my:special:path#meta/root.cm");
        assert!(address.is_absolute());
        assert_eq!(address.scheme(), "custom-resolver");
        assert_eq!(address.path(), "my:special:path");
        assert_eq!(address.query(), None);
        assert_eq!(address.resource(), Some("meta/root.cm"));
        assert_eq!(address.url(), "custom-resolver:my:special:path#meta/root.cm");
        assert_matches!(
            address.to_url_and_context(),
            ("custom-resolver:my:special:path#meta/root.cm", None)
        );

        let address = from_absolute_url("cast:00000000");
        assert!(address.is_absolute());
        assert_eq!(address.scheme(), "cast");
        assert_eq!(address.path(), "00000000");
        assert_eq!(address.query(), None);
        assert_eq!(address.resource(), None);
        assert_eq!(address.url(), "cast:00000000");
        assert_matches!(address.to_url_and_context(), ("cast:00000000", None));

        let address = from_absolute_url("cast:00000000#meta/root.cm");
        assert!(address.is_absolute());
        assert_eq!(address.scheme(), "cast");
        assert_eq!(address.path(), "00000000");
        assert_eq!(address.query(), None);
        assert_eq!(address.resource(), Some("meta/root.cm"));
        assert_eq!(address.url(), "cast:00000000#meta/root.cm");
        assert_matches!(address.to_url_and_context(), ("cast:00000000#meta/root.cm", None));

        let address =
            from_absolute_url("fuchsia-pkg://fuchsia.com/package?hash=cafe0123#meta/comp.cm");
        assert!(address.is_absolute());
        assert_eq!(address.scheme(), "fuchsia-pkg");
        assert_eq!(address.path(), "/package");
        assert_eq!(address.resource(), Some("meta/comp.cm"));
        assert_eq!(address.query(), Some("hash=cafe0123"));
        assert_eq!(address.url(), "fuchsia-pkg://fuchsia.com/package?hash=cafe0123#meta/comp.cm");
        assert_matches!(
            address.to_url_and_context(),
            ("fuchsia-pkg://fuchsia.com/package?hash=cafe0123#meta/comp.cm", None)
        );
    }

    #[test]
    fn test_relative_path() {
        let url = Url::parse("relative:///package#fragment").unwrap();
        assert_eq!(url.path(), "/package");
        assert_eq!(ComponentAddress::relative_path(&url), "package");

        let url = Url::parse("cast:00000000#fragment").unwrap();
        assert_eq!(url.path(), "00000000");
        assert_eq!(ComponentAddress::relative_path(&url), "00000000");
    }

    #[test]
    fn test_parse_relative_url() {
        let relative_prefix_with_one_less_slash = Url::parse("relative://").unwrap();
        assert_eq!(relative_prefix_with_one_less_slash.scheme(), "relative");
        assert_eq!(relative_prefix_with_one_less_slash.host(), None);
        assert_eq!(relative_prefix_with_one_less_slash.path(), "");

        assert_eq!(RELATIVE_URL_BASE.scheme(), "relative");
        assert_eq!(RELATIVE_URL_BASE.host(), None);
        assert_eq!(RELATIVE_URL_BASE.path(), "/");

        let mut clone_relative_base = RELATIVE_URL_BASE.clone();
        assert_eq!(clone_relative_base.path(), "/");
        clone_relative_base.set_path("");
        assert_eq!(clone_relative_base.path(), "");

        let mut clone_relative_base = RELATIVE_URL_BASE.clone();
        assert_eq!(clone_relative_base.path(), "/");
        clone_relative_base.set_path("some_path_no_initial_slash");
        assert_eq!(clone_relative_base.path(), "/some_path_no_initial_slash");

        let clone_relative_base = RELATIVE_URL_BASE.clone();
        let joined = clone_relative_base.join("some_path_no_initial_slash").unwrap();
        assert_eq!(joined.path(), "/some_path_no_initial_slash");

        let clone_relative_base = relative_prefix_with_one_less_slash.clone();
        let joined = clone_relative_base.join("some_path_no_initial_slash").unwrap();
        // Same result as with three slashes
        assert_eq!(joined.path(), "/some_path_no_initial_slash");

        let relative_url = parse_relative_url("subpackage#meta/subcomp.cm");
        assert_eq!(relative_url.path(), "/subpackage");
        assert_eq!(relative_url.query(), None);
        assert_eq!(relative_url.fragment(), Some("meta/subcomp.cm"));

        let relative_url = parse_relative_url("/subpackage#meta/subcomp.cm");
        assert_eq!(relative_url.path(), "/subpackage");
        assert_eq!(relative_url.query(), None);
        assert_eq!(relative_url.fragment(), Some("meta/subcomp.cm"));

        let relative_url = parse_relative_url("//subpackage#meta/subcomp.cm");
        assert_eq!(relative_url.path(), "");
        assert_eq!(relative_url.host_str(), Some("subpackage"));
        assert_eq!(relative_url.query(), None);
        assert_eq!(relative_url.fragment(), Some("meta/subcomp.cm"));

        let relative_url = parse_relative_url("///subpackage#meta/subcomp.cm");
        assert_eq!(relative_url.path(), "/subpackage");
        assert_eq!(relative_url.host_str(), None);
        assert_eq!(relative_url.query(), None);
        assert_eq!(relative_url.fragment(), Some("meta/subcomp.cm"));

        let relative_url = parse_relative_url("fuchsia.com/subpackage#meta/subcomp.cm");
        assert_eq!(relative_url.path(), "/fuchsia.com/subpackage");
        assert_eq!(relative_url.query(), None);
        assert_eq!(relative_url.fragment(), Some("meta/subcomp.cm"));

        let relative_url = parse_relative_url("//fuchsia.com/subpackage#meta/subcomp.cm");
        assert_eq!(relative_url.path(), "/subpackage");
        assert_eq!(relative_url.host_str(), Some("fuchsia.com"));
        assert_eq!(relative_url.query(), None);
        assert_eq!(relative_url.fragment(), Some("meta/subcomp.cm"));

        assert_matches!(
            ComponentAddress::parse_relative_url(
                &"fuchsia-pkg://fuchsia.com/subpackage#meta/subcomp.cm".parse().unwrap()
            ),
            Err(ResolverError::MalformedUrl(..))
        );

        let relative_url = parse_relative_url("#meta/peercomp.cm");
        assert_eq!(relative_url.path(), "/");
        assert_eq!(relative_url.query(), None);
        assert_eq!(relative_url.fragment(), Some("meta/peercomp.cm"));

        let address = from_absolute_url("some-scheme://fuchsia.com/package#meta/comp.cm")
            .clone_with_new_resource(relative_url.fragment())
            .unwrap();

        assert!(address.is_absolute());
        assert_eq!(address.scheme(), "some-scheme");
        assert_eq!(address.path(), "/package");
        assert_eq!(address.query(), None);
        assert_eq!(address.resource(), Some("meta/peercomp.cm"));
        assert_eq!(address.url(), "some-scheme://fuchsia.com/package#meta/peercomp.cm");

        let address = from_absolute_url("cast:00000000")
            .clone_with_new_resource(relative_url.fragment())
            .unwrap();

        assert!(address.is_absolute());
        assert_eq!(address.scheme(), "cast");
        assert_eq!(address.path(), "00000000");
        assert_eq!(address.query(), None);
        assert_eq!(address.resource(), Some("meta/peercomp.cm"));
        assert_eq!(address.url(), "cast:00000000#meta/peercomp.cm");
    }

    static COMPONENT_DECL: LazyLock<cm_rust::ComponentDecl> = LazyLock::new(|| {
        new_decl_from_json(json!(
        {
            "include": [ "syslog/client.shard.cml" ],
            "program": {
                "runner": "elf",
                "binary": "bin/example",
            },
            "children": [
                {
                    "name": "logger",
                    "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                    "environment": "#env_one",
                },
            ],
            "collections": [
                {
                    "name": "modular",
                    "durability": "transient",
                },
            ],
            "capabilities": [
                {
                    "protocol": "fuchsia.logger.Log2",
                    "path": "/svc/fuchsia.logger.Log2",
                },
            ],
            "use": [
                {
                    "protocol": "fuchsia.fonts.LegacyProvider",
                },
            ],
            "environments": [
                {
                    "name": "env_one",
                    "extends": "none",
                    "__stop_timeout_ms": 1337,
                },
            ],
            "facets": {
                "author": "Fuchsia",
            }}))
        .expect("failed to construct manifest")
    });

    #[fuchsia::test]
    fn test_read_and_validate_manifest() {
        let manifest = fmem::Data::Bytes(
            fidl::persist(&COMPONENT_DECL.clone().native_into_fidl())
                .expect("failed to encode manifest"),
        );
        let actual = read_and_validate_manifest(&manifest, &mut DirectedGraph::new())
            .expect("failed to decode manifest");
        assert_eq!(actual, COMPONENT_DECL.clone());
    }

    #[fuchsia::test]
    async fn test_read_and_validate_config_values() {
        let fidl_config_values = fdecl::ConfigValuesData {
            values: Some(vec![
                fdecl::ConfigValueSpec {
                    value: Some(fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::Bool(false))),
                    ..Default::default()
                },
                fdecl::ConfigValueSpec {
                    value: Some(fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::Uint8(5))),
                    ..Default::default()
                },
                fdecl::ConfigValueSpec {
                    value: Some(fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::String(
                        "hello!".to_string(),
                    ))),
                    ..Default::default()
                },
                fdecl::ConfigValueSpec {
                    value: Some(fdecl::ConfigValue::Vector(fdecl::ConfigVectorValue::BoolVector(
                        vec![true, false],
                    ))),
                    ..Default::default()
                },
                fdecl::ConfigValueSpec {
                    value: Some(fdecl::ConfigValue::Vector(
                        fdecl::ConfigVectorValue::StringVector(vec![
                            "hello!".to_string(),
                            "world!".to_string(),
                        ]),
                    )),
                    ..Default::default()
                },
            ]),
            checksum: Some(fdecl::ConfigChecksum::Sha256([0; 32])),
            ..Default::default()
        };
        let config_values = cm_rust::ConfigValuesData {
            values: Box::from([
                cm_rust::ConfigValueSpec {
                    value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::Bool(false)),
                },
                cm_rust::ConfigValueSpec {
                    value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::Uint8(5)),
                },
                cm_rust::ConfigValueSpec {
                    value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::String(
                        "hello!".into(),
                    )),
                },
                cm_rust::ConfigValueSpec {
                    value: cm_rust::ConfigValue::Vector(cm_rust::ConfigVectorValue::BoolVector(
                        Box::from([true, false]),
                    )),
                },
                cm_rust::ConfigValueSpec {
                    value: cm_rust::ConfigValue::Vector(cm_rust::ConfigVectorValue::StringVector(
                        Box::from(["hello!".into(), "world!".into()]),
                    )),
                },
            ]),
            checksum: cm_rust::ConfigChecksum::Sha256([0; 32]),
        };
        let data = fmem::Data::Bytes(
            fidl::persist(&fidl_config_values).expect("failed to encode config values"),
        );
        let actual =
            read_and_validate_config_values(&data).expect("failed to decode config values");
        assert_eq!(actual, config_values);
    }

    #[derive(Debug, Default, Clone)]
    struct MockTopInstance {
        namespace_capabilities: NamespaceCapabilities,
        builtin_capabilities: BuiltinCapabilities,
    }

    impl TopInstanceInterface for MockTopInstance {
        fn namespace_capabilities(&self) -> &NamespaceCapabilities {
            &self.namespace_capabilities
        }
        fn builtin_capabilities(&self) -> &BuiltinCapabilities {
            &self.builtin_capabilities
        }
    }

    #[derive(Clone)]
    struct MockComponentInstance {
        parent: Option<Box<MockComponentInstance>>,
        resolved_state: Option<MockResolvedState>,
        moniker: Moniker,
        address: cm_types::Url,
    }
    #[async_trait]
    impl ComponentInstanceInterface for MockComponentInstance {
        type TopInstance = MockTopInstance;
        fn moniker(&self) -> &Moniker {
            &self.moniker
        }
        fn url(&self) -> &cm_types::Url {
            &self.address
        }
        fn config_parent_overrides(&self) -> Option<&[cm_rust::ConfigOverride]> {
            unimplemented!()
        }
        fn policy_checker(&self) -> &GlobalPolicyChecker {
            unimplemented!()
        }
        fn component_id_index(&self) -> &component_id_index::Index {
            unimplemented!()
        }
        fn try_get_parent(
            &self,
        ) -> Result<ExtendedInstanceInterface<Self>, ComponentInstanceError> {
            if let Some(parent) = self.parent.as_ref() {
                Ok(ExtendedInstanceInterface::Component(Arc::new((**parent).clone())))
            } else {
                Ok(ExtendedInstanceInterface::AboveRoot(Arc::new(MockTopInstance::default())))
            }
        }
        async fn lock_resolved_state<'a>(
            self: &'a Arc<Self>,
        ) -> Result<Box<dyn ResolvedInstanceInterface<Component = Self> + 'a>, ComponentInstanceError>
        {
            Ok(Box::new(self.resolved_state.as_ref().unwrap()))
        }
        async fn component_sandbox(
            self: &Arc<Self>,
        ) -> Result<ComponentSandbox, ComponentInstanceError> {
            unimplemented!()
        }
    }
    impl MockComponentInstance {
        async fn component_address(self: &Arc<Self>) -> Result<ComponentAddress, ResolverError> {
            ComponentAddress::from_url(&self.address, self).await
        }
    }

    #[derive(Clone)]
    struct MockResolvedState {
        address: Result<ComponentAddress, ResolverError>,
        context_to_resolve_children: Option<ComponentResolutionContext>,
    }
    #[async_trait]
    impl ResolvedInstanceInterface for MockResolvedState {
        type Component = MockComponentInstance;
        fn try_uses(&self) -> Option<Box<[UseDecl]>> {
            unimplemented!()
        }
        fn try_exposes(&self) -> Option<Box<[ExposeDecl]>> {
            unimplemented!()
        }
        fn try_offers(&self) -> Option<Box<[OfferDecl]>> {
            unimplemented!()
        }
        fn try_capabilities(&self) -> Option<Box<[CapabilityDecl]>> {
            unimplemented!()
        }
        fn try_collections(&self) -> Option<Box<[CollectionDecl]>> {
            unimplemented!()
        }
        fn get_child(&self, _moniker: &BorrowedChildName) -> Option<Arc<Self::Component>> {
            unimplemented!()
        }
        fn children_in_collection(
            &self,
            _collection: &Name,
        ) -> Vec<(ChildName, Arc<Self::Component>)> {
            unimplemented!()
        }
        async fn address(&self) -> Result<ComponentAddress, ResolverError> {
            self.address.clone()
        }
        fn context_to_resolve_children(&self) -> Option<ComponentResolutionContext> {
            self.context_to_resolve_children.clone()
        }
    }

    #[fuchsia::test]
    async fn test_from_absolute_component_url_with_component_instance() -> Result<(), Error> {
        let url_str = "fuchsia-pkg://fuchsia.com/package#meta/comp.cm";
        let root = Arc::new(MockComponentInstance {
            parent: None,
            resolved_state: Some(MockResolvedState {
                address: Ok(ComponentAddress::new_absolute(Url::parse(url_str).unwrap())),
                context_to_resolve_children: None,
            }),
            moniker: Moniker::root(),
            address: cm_types::Url::new(url_str).unwrap(),
        });

        let abs = root.component_address().await.unwrap();
        assert_matches!(abs, ComponentAddress::Absolute { .. });
        assert_eq!(abs.scheme(), "fuchsia-pkg");
        assert_eq!(abs.path(), "/package");
        assert_eq!(abs.resource(), Some("meta/comp.cm"));
        Ok(())
    }

    #[fuchsia::test]
    async fn test_from_relative_path_component_url_with_component_instance() -> Result<(), Error> {
        let root_url_str = "fuchsia-pkg://fuchsia.com/package#meta/comp.cm";
        let root = MockComponentInstance {
            parent: None,
            resolved_state: Some(MockResolvedState {
                address: Ok(ComponentAddress::new_absolute(Url::parse(root_url_str).unwrap())),
                context_to_resolve_children: Some(ComponentResolutionContext::new(
                    "package_context".as_bytes().to_vec(),
                )),
            }),
            moniker: Moniker::root(),
            address: cm_types::Url::new(root_url_str).unwrap(),
        };
        let child = Arc::new(MockComponentInstance {
            parent: Some(Box::new(root)),
            resolved_state: None,
            moniker: "/child".try_into().unwrap(),
            address: cm_types::Url::new("subpackage#meta/subcomp.cm").unwrap(),
        });

        let relpath = child.component_address().await.unwrap();
        assert_matches!(relpath, ComponentAddress::RelativePath { .. });
        assert_eq!(relpath.path(), "subpackage");
        assert_eq!(relpath.resource(), Some("meta/subcomp.cm"));
        assert_eq!(
            relpath.context(),
            &ComponentResolutionContext::new("package_context".as_bytes().to_vec())
        );

        Ok(())
    }

    #[fuchsia::test]
    async fn test_from_relative_path_component_url_with_cast_component_instance()
    -> Result<(), Error> {
        let root_url_str = "cast:00000000/package#meta/comp.cm";
        let root = MockComponentInstance {
            parent: None,
            resolved_state: Some(MockResolvedState {
                address: Ok(ComponentAddress::new_absolute(Url::parse(root_url_str).unwrap())),
                context_to_resolve_children: Some(ComponentResolutionContext::new(
                    "package_context".as_bytes().to_vec(),
                )),
            }),
            moniker: Moniker::root(),
            address: cm_types::Url::new(root_url_str).unwrap(),
        };
        let child = Arc::new(MockComponentInstance {
            parent: Some(Box::new(root)),
            resolved_state: None,
            moniker: "/child".try_into().unwrap(),
            address: cm_types::Url::new("subpackage#meta/subcomp.cm").unwrap(),
        });

        let relpath = child.component_address().await.unwrap();
        assert_matches!(relpath, ComponentAddress::RelativePath { .. });
        assert_eq!(relpath.path(), "subpackage");
        assert_eq!(relpath.resource(), Some("meta/subcomp.cm"));
        assert_eq!(
            relpath.context(),
            &ComponentResolutionContext::new("package_context".as_bytes().to_vec())
        );

        Ok(())
    }
}
