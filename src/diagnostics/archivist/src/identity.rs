// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use diagnostics_message::MonikerWithUrl;
use flyweights::FlyStr;
use moniker::{ExtendedMoniker, Moniker};
use std::hash::{Hash, Hasher};

#[derive(Debug)]
pub struct ComponentIdentity {
    /// Moniker of the component that this artifacts container is representing.
    pub moniker: ExtendedMoniker,

    /// The url with which the associated component was launched.
    pub url: FlyStr,
}

// Just use the moniker when hashing and checking equality. That's the unique ID we need. The URL
// is just used for metadata purposes. Typically both can be used and we can
// derive(ComponentIdentity), but as long as we have the workaround for netstack2 diagnostics we
// should just use the moniker as we don't know the runtime URL for this component.
// TODO(https://fxbug.dev/324494668): consider just derive(Hash) when Netstack2 is gone.
impl Hash for ComponentIdentity {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.moniker.hash(state);
    }
}

impl PartialEq for ComponentIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.moniker == other.moniker
    }
}

impl PartialEq<Moniker> for ComponentIdentity {
    fn eq(&self, other: &Moniker) -> bool {
        matches!(&self.moniker, ExtendedMoniker::ComponentInstance(moniker) if moniker == other)
    }
}

impl Eq for ComponentIdentity {}

impl ComponentIdentity {
    pub fn new(moniker: ExtendedMoniker, url: impl Into<FlyStr>) -> Self {
        ComponentIdentity { moniker, url: url.into() }
    }

    /// Returns generic metadata, suitable for providing a uniform ID to unattributed data.
    pub fn unknown() -> Self {
        Self::new(
            ExtendedMoniker::parse_str("/UNKNOWN").expect("Unknown is valid"),
            "fuchsia-pkg://UNKNOWN",
        )
    }
}

#[cfg(test)]
impl From<Vec<&str>> for ComponentIdentity {
    fn from(moniker_segments: Vec<&str>) -> Self {
        let moniker = moniker::Moniker::try_from(moniker_segments.as_slice()).unwrap();
        Self { moniker: ExtendedMoniker::from(moniker), url: "".into() }
    }
}

impl From<ComponentIdentity> for MonikerWithUrl {
    fn from(identity: ComponentIdentity) -> Self {
        Self { moniker: identity.moniker, url: identity.url }
    }
}

impl From<&ComponentIdentity> for MonikerWithUrl {
    fn from(identity: &ComponentIdentity) -> Self {
        Self { moniker: identity.moniker.clone(), url: identity.url.clone() }
    }
}

impl std::fmt::Display for ComponentIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.moniker.fmt(f)
    }
}
