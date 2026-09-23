// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fmt::Display as _;

/// Sealed trait for URL schemes.
/// Allows having separate types for URLs with schemes that are more constrained than
/// ['Scheme`](crate::Scheme).
#[allow(private_bounds)]
pub trait SchemeTrait: Sized + std::fmt::Display + std::clone::Clone + crate::Sealer {
    fn try_from_part(scheme: crate::Scheme) -> Result<Self, crate::ParseError>;
}

/// Sealed trait for URL hosts.
/// Allows having separate types for URLs with and without hosts.
#[allow(private_bounds)]
pub trait HostTrait: Sized + std::fmt::Display + std::clone::Clone + crate::Sealer {
    fn try_from_part(host: Option<crate::Host>) -> Result<Self, crate::ParseError>;
}

/// Sealed trait for URL paths.
/// Allows having separate types for URLs with paths that are more constrained than
/// [`Path`](crate::Path).
#[allow(private_bounds)]
pub trait PathTrait: Sized + std::clone::Clone + crate::Sealer {
    fn try_from_part(path: Option<crate::Path>) -> Result<Self, crate::ParseError>;
    fn is_present(&self) -> bool;
    fn url_display(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result;
}

/// Sealed trait for URL hashes.
/// Allows having separate types for URLs with and without hashes (called pinned and unpinned).
#[allow(private_bounds)]
pub trait HashTrait: Sized + std::clone::Clone + crate::Sealer {
    fn try_from_part(hash: Option<crate::Hash>) -> Result<Self, crate::ParseError>;
    fn is_present(&self) -> bool;
    fn url_display(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result;
}

/// Building block for creating component URL types. To use create implementations of the type
/// variables that behave as desired and use them in a type alias.
#[derive(Debug, Clone)]
pub enum ComponentUrl<SCHEME, HOST, PATH, HASH> {
    Absolute(AbsoluteComponentUrl<SCHEME, HOST, PATH, HASH>),
    Relative(crate::RelativeComponentUrl),
}

impl<SCHEME, HOST, PATH, HASH> ComponentUrl<SCHEME, HOST, PATH, HASH>
where
    SCHEME: SchemeTrait,
    HOST: HostTrait,
    PATH: PathTrait,
    HASH: HashTrait,
{
    /// Create a URL from a `&str`.
    pub fn parse(url: &str) -> Result<Self, crate::ParseError> {
        let parts = crate::UrlParts::parse(url)?;
        Ok(if parts.scheme.is_some() {
            Self::Absolute(AbsoluteComponentUrl::try_from_parts(parts)?)
        } else {
            Self::Relative(crate::RelativeComponentUrl::from_parts(parts)?)
        })
    }

    /// Obtain a reference to the URL's resource.
    pub fn resource(&self) -> &crate::Resource {
        match self {
            Self::Absolute(absolute) => &absolute.resource,
            Self::Relative(relative) => &relative.resource(),
        }
    }

    /// Create a package URL from this URL. Equivalent to this URL without the resource.
    pub fn to_package_url(&self) -> PackageUrl<SCHEME, HOST, PATH, HASH> {
        match self {
            Self::Absolute(absolute) => PackageUrl::Absolute(absolute.to_package_url()),
            Self::Relative(relative) => PackageUrl::Relative(relative.package_url().clone()),
        }
    }
}

impl<SCHEME, HOST, PATH, HASH> std::fmt::Display for ComponentUrl<SCHEME, HOST, PATH, HASH>
where
    SCHEME: SchemeTrait,
    HOST: HostTrait,
    PATH: PathTrait,
    HASH: HashTrait,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Absolute(absolute) => absolute.fmt(f),
            Self::Relative(relative) => relative.fmt(f),
        }
    }
}

/// Building block for creating component URL types. To use create implementations of the type
/// variables that behave as desired and use them in a type alias.
#[derive(Debug, Clone)]
pub struct AbsoluteComponentUrl<SCHEME, HOST, PATH, HASH> {
    scheme: SCHEME,
    host: HOST,
    path: PATH,
    hash: HASH,
    resource: crate::Resource,
}

impl<SCHEME, HOST, PATH, HASH> AbsoluteComponentUrl<SCHEME, HOST, PATH, HASH>
where
    SCHEME: SchemeTrait,
    HOST: HostTrait,
    PATH: PathTrait,
    HASH: HashTrait,
{
    /// Create a URL from a `&str`.
    pub fn parse(url: &str) -> Result<Self, crate::ParseError> {
        let parts = crate::UrlParts::parse(url)?;
        Self::try_from_parts(parts)
    }

    /// Creates a URL from its constituent parts.
    pub fn from_parts(
        scheme: SCHEME,
        host: HOST,
        path: PATH,
        hash: HASH,
        resource: crate::Resource,
    ) -> Self {
        Self { scheme, host, path, hash, resource }
    }

    fn try_from_parts(parts: crate::UrlParts) -> Result<Self, crate::ParseError> {
        let crate::UrlParts { scheme, host, path, hash, resource } = parts;
        let scheme = SCHEME::try_from_part(scheme.ok_or(crate::ParseError::MissingScheme)?)?;
        let host = HOST::try_from_part(host)?;
        let path = PATH::try_from_part(path)?;
        let hash = HASH::try_from_part(hash)?;
        let Some(resource) = resource else {
            return Err(crate::ParseError::MissingResource);
        };
        Ok(Self { scheme, host, path, hash, resource })
    }

    /// Obtain a reference to the URL's path.
    pub fn path(&self) -> &PATH {
        &self.path
    }

    /// Obtain a reference to the URL's resource.
    pub fn resource(&self) -> &crate::Resource {
        &self.resource
    }

    /// Create an [`AbsolutePackageUrl`] from this URL, which is equal to this URL without its
    /// [`Resource`](crate::Resource).
    pub fn to_package_url(&self) -> AbsolutePackageUrl<SCHEME, HOST, PATH, HASH> {
        AbsolutePackageUrl::<SCHEME, HOST, PATH, HASH> {
            scheme: self.scheme.clone(),
            host: self.host.clone(),
            path: self.path.clone(),
            hash: self.hash.clone(),
        }
    }
}

impl<SCHEME, HOST, PATH, HASH> std::fmt::Display for AbsoluteComponentUrl<SCHEME, HOST, PATH, HASH>
where
    SCHEME: SchemeTrait,
    HOST: HostTrait,
    PATH: PathTrait,
    HASH: HashTrait,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let () = write!(f, "{}://{}", self.scheme, self.host)?;
        if self.path.is_present() {
            let () = f.write_str("/")?;
            let () = self.path.url_display(f)?;
        }
        if self.hash.is_present() {
            let () = f.write_str("?hash=")?;
            let () = self.hash.url_display(f)?;
        }
        let () = write!(f, "#{}", self.resource)?;
        Ok(())
    }
}

/// Building block for creating package URL types. To use create implementations of the type
/// variables that behave as desired and use them in a type alias.
pub enum PackageUrl<SCHEME, HOST, PATH, HASH> {
    Absolute(AbsolutePackageUrl<SCHEME, HOST, PATH, HASH>),
    Relative(crate::RelativePackageUrl),
}

impl<SCHEME, HOST, PATH, HASH> PackageUrl<SCHEME, HOST, PATH, HASH>
where
    SCHEME: SchemeTrait,
    HOST: HostTrait,
    PATH: PathTrait,
    HASH: HashTrait,
{
    /// Create a URL from a `&str`.
    pub fn parse(url: &str) -> Result<Self, crate::ParseError> {
        let parts = crate::UrlParts::parse(url)?;
        Ok(if parts.scheme.is_some() {
            Self::Absolute(AbsolutePackageUrl::try_from_parts(parts)?)
        } else {
            Self::Relative(crate::RelativePackageUrl::from_parts(parts)?)
        })
    }
}

impl<SCHEME, HOST, PATH, HASH> std::fmt::Display for PackageUrl<SCHEME, HOST, PATH, HASH>
where
    SCHEME: SchemeTrait,
    HOST: HostTrait,
    PATH: PathTrait,
    HASH: HashTrait,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Absolute(absolute) => absolute.fmt(f),
            Self::Relative(relative) => relative.fmt(f),
        }
    }
}

/// Building block for creating package URL types. To use create implementations of the type
/// variables that behave as desired and use them in a type alias.
#[derive(Debug, Clone)]
pub struct AbsolutePackageUrl<SCHEME, HOST, PATH, HASH> {
    scheme: SCHEME,
    host: HOST,
    path: PATH,
    hash: HASH,
}

impl<SCHEME, HOST, PATH, HASH> AbsolutePackageUrl<SCHEME, HOST, PATH, HASH>
where
    SCHEME: SchemeTrait,
    HOST: HostTrait,
    PATH: PathTrait,
    HASH: HashTrait,
{
    /// Create a URL from a `&str`.
    pub fn parse(url: &str) -> Result<Self, crate::ParseError> {
        let parts = crate::UrlParts::parse(url)?;
        Self::try_from_parts(parts)
    }

    fn try_from_parts(parts: crate::UrlParts) -> Result<Self, crate::ParseError> {
        let crate::UrlParts { scheme, host, path, hash, resource } = parts;
        let scheme = SCHEME::try_from_part(scheme.ok_or(crate::ParseError::MissingScheme)?)?;
        let host = HOST::try_from_part(host)?;
        let path = PATH::try_from_part(path)?;
        let hash = HASH::try_from_part(hash)?;
        if resource.is_some() {
            return Err(crate::ParseError::CannotContainResource);
        }
        Ok(Self { scheme, host, path, hash })
    }

    /// Obtain a reference to the URL's path.
    pub fn path(&self) -> &PATH {
        &self.path
    }
}

impl<SCHEME, HOST, PATH, HASH> std::fmt::Display for AbsolutePackageUrl<SCHEME, HOST, PATH, HASH>
where
    SCHEME: SchemeTrait,
    HOST: HostTrait,
    PATH: PathTrait,
    HASH: HashTrait,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let () = write!(f, "{}://{}", self.scheme, self.host)?;
        if self.path.is_present() {
            let () = f.write_str("/")?;
            let () = self.path.url_display(f)?;
        }
        if self.hash.is_present() {
            let () = f.write_str("?hash=")?;
            let () = self.hash.url_display(f)?;
        }
        Ok(())
    }
}

/// Type for URLs that do not have any additional constraints on their scheme.
impl crate::Sealer for crate::Scheme {}
impl SchemeTrait for crate::Scheme {
    fn try_from_part(scheme: crate::Scheme) -> Result<Self, crate::ParseError> {
        Ok(scheme)
    }
}

/// Type for URLs that do not have a host.
#[derive(Debug, Clone)]
pub struct NoneHost;
impl crate::Sealer for NoneHost {}
impl HostTrait for NoneHost {
    fn try_from_part(host: Option<crate::Host>) -> Result<Self, crate::ParseError> {
        match host {
            None => Ok(Self),
            _ => Err(crate::ParseError::HostMustBeEmpty),
        }
    }
}
impl std::fmt::Display for NoneHost {
    fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Ok(())
    }
}

/// Type for URLs that might have a host.
#[derive(Debug, Clone)]
pub struct OptionHost(Option<crate::Host>);
impl crate::Sealer for OptionHost {}
impl HostTrait for OptionHost {
    fn try_from_part(host: Option<crate::Host>) -> Result<Self, crate::ParseError> {
        Ok(Self(host))
    }
}
impl std::fmt::Display for OptionHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(host) = &self.0 { host.fmt(f) } else { Ok(()) }
    }
}

impl crate::Sealer for Option<crate::Path> {}
impl PathTrait for Option<crate::Path> {
    fn try_from_part(path: Option<crate::Path>) -> Result<Self, crate::ParseError> {
        Ok(path)
    }
    fn is_present(&self) -> bool {
        self.is_some()
    }
    fn url_display(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(path) = &self { path.fmt(f) } else { Ok(()) }
    }
}

/// Type for URLs that do not have a hash (are unpinned).
#[derive(Debug, Clone)]
pub struct NoneHash;
impl crate::Sealer for NoneHash {}
impl HashTrait for NoneHash {
    fn try_from_part(hash: Option<crate::Hash>) -> Result<Self, crate::ParseError> {
        match hash {
            None => Ok(Self),
            _ => Err(crate::ParseError::CannotContainHash),
        }
    }
    fn is_present(&self) -> bool {
        false
    }
    fn url_display(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Ok(())
    }
}

impl crate::Sealer for Option<crate::Hash> {}
impl HashTrait for Option<crate::Hash> {
    fn try_from_part(hash: Option<crate::Hash>) -> Result<Self, crate::ParseError> {
        Ok(hash)
    }
    fn is_present(&self) -> bool {
        self.is_some()
    }
    fn url_display(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(hash) = &self { hash.fmt(f) } else { Ok(()) }
    }
}
