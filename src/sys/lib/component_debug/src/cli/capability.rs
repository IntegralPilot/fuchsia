// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::capability::get_all_route_segments;
use anyhow::Result;
use cm_rust::{ExposeDeclCommon, OfferDeclCommon, SourceName, UseDeclCommon};
use flex_fuchsia_sys2 as fsys;
use moniker::Moniker;

#[cfg(feature = "serde")]
use {
    schemars::JsonSchema,
    serde::{Deserialize, Serialize},
};

#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize, JsonSchema),
    serde(tag = "type", rename_all = "snake_case")
)]
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum RouteSegment {
    /// The capability was used by a component instance in its manifest.
    UseBy {
        /// The moniker of the component using the capability.
        moniker: Moniker,
        /// The source name of the capability being used.
        capability: String,
        /// The source location the capability is used from (e.g. "parent", "self", "framework").
        source: String,
    },

    /// The capability was offered by a component instance in its manifest.
    OfferBy {
        /// The moniker of the component offering the capability.
        moniker: Moniker,
        /// The source name of the capability being offered.
        capability: String,
        /// The source location offering the capability (e.g. "self", "parent", "#child").
        source: String,
        /// The target child or collection receiving the offer (e.g. "#child").
        target: String,
    },

    /// The capability was exposed by a component instance in its manifest.
    ExposeBy {
        /// The moniker of the component exposing the capability.
        moniker: Moniker,
        /// The source name of the capability being exposed.
        capability: String,
        /// The source location of the exposed capability (e.g. "self", "#child").
        source: String,
        /// The target destination of the exposed capability (e.g. "parent", "framework").
        target: String,
    },

    /// The capability was declared by a component instance in its manifest.
    DeclareBy {
        /// The moniker of the component declaring the capability.
        moniker: Moniker,
        /// The name of the declared capability.
        capability: String,
    },
}

impl From<crate::capability::RouteSegment> for RouteSegment {
    fn from(segment: crate::capability::RouteSegment) -> Self {
        match segment {
            crate::capability::RouteSegment::UseBy { moniker, capability } => Self::UseBy {
                moniker,
                capability: capability.source_name().to_string(),
                source: capability.source().to_string(),
            },
            crate::capability::RouteSegment::OfferBy { moniker, capability } => Self::OfferBy {
                moniker,
                capability: capability.source_name().to_string(),
                source: capability.source().to_string(),
                target: capability.target().to_string(),
            },
            crate::capability::RouteSegment::ExposeBy { moniker, capability } => Self::ExposeBy {
                moniker,
                capability: capability.source_name().to_string(),
                source: capability.source().to_string(),
                target: capability.target().to_string(),
            },
            crate::capability::RouteSegment::DeclareBy { moniker, capability } => {
                Self::DeclareBy { moniker, capability: capability.name().to_string() }
            }
        }
    }
}

impl std::fmt::Display for RouteSegment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UseBy { moniker, capability, source } => {
                write!(f, "`{moniker}` used `{capability}` from {source}")
            }
            Self::OfferBy { moniker, capability, source, target } => {
                write!(f, "`{moniker}` offered `{capability}` from {source} to {target}")
            }
            Self::ExposeBy { moniker, capability, source, target } => {
                write!(f, "`{moniker}` exposed `{capability}` from {source} to {target}")
            }
            Self::DeclareBy { moniker, capability } => {
                write!(f, "`{moniker}` declared capability `{capability}`")
            }
        }
    }
}

pub async fn capability_cmd_serialized(
    query: String,
    realm_query: fsys::RealmQueryProxy,
) -> Result<Vec<RouteSegment>> {
    let segments = get_all_route_segments(query, &realm_query).await?;
    Ok(segments.into_iter().map(Into::into).collect())
}

pub async fn capability_cmd_print<W: std::io::Write>(
    query: String,
    realm_query: fsys::RealmQueryProxy,
    mut writer: W,
) -> Result<()> {
    let segments = capability_cmd_serialized(query, realm_query).await?;

    let mut decls = vec![];
    let mut exposes = vec![];
    let mut offers = vec![];
    let mut uses = vec![];

    for s in segments {
        match &s {
            RouteSegment::DeclareBy { .. } => decls.push(s),
            RouteSegment::ExposeBy { .. } => exposes.push(s),
            RouteSegment::OfferBy { .. } => offers.push(s),
            RouteSegment::UseBy { .. } => uses.push(s),
        }
    }

    if decls.is_empty() {
        writeln!(writer, "Declarations: None")?;
    } else {
        writeln!(writer, "Declarations:")?;
        for decl in decls {
            writeln!(writer, "  {}", decl)?;
        }
    }

    writeln!(writer, "")?;

    if exposes.is_empty() {
        writeln!(writer, "Exposes: None")?;
    } else {
        writeln!(writer, "Exposes:")?;
        for decl in exposes {
            writeln!(writer, "  {}", decl)?;
        }
    }

    writeln!(writer, "")?;

    if offers.is_empty() {
        writeln!(writer, "Offers: None")?;
    } else {
        writeln!(writer, "Offers:")?;
        for decl in offers {
            writeln!(writer, "  {}", decl)?;
        }
    }

    writeln!(writer, "")?;

    if uses.is_empty() {
        writeln!(writer, "Uses: None")?;
    } else {
        writeln!(writer, "Uses:")?;
        for decl in uses {
            writeln!(writer, "  {}", decl)?;
        }
    }

    Ok(())
}

pub async fn capability_cmd<W: std::io::Write>(
    query: String,
    realm_query: fsys::RealmQueryProxy,
    writer: W,
) -> Result<()> {
    capability_cmd_print(query, realm_query, writer).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use cm_rust::{
        CapabilityDecl, ExposeDecl, ExposeSource, ExposeTarget, OfferDecl, OfferSource,
        ProtocolDecl, UseDecl, UseSource,
    };
    use cm_rust_testing::{ExposeBuilder, OfferBuilder, UseBuilder};
    use cm_types::{DeliveryType, Name};

    #[test]
    fn test_route_segment_conversion_and_display() {
        let moniker = Moniker::parse_str("foo/bar").unwrap();

        let use_decl: UseDecl =
            UseBuilder::protocol().name("fuchsia.foo.Bar").source(UseSource::Parent).build();
        let seg: RouteSegment = crate::capability::RouteSegment::UseBy {
            moniker: moniker.clone(),
            capability: use_decl,
        }
        .into();
        assert_eq!(
            seg,
            RouteSegment::UseBy {
                moniker: moniker.clone(),
                capability: "fuchsia.foo.Bar".to_string(),
                source: "parent".to_string(),
            }
        );
        assert_eq!(format!("{seg}"), "`foo/bar` used `fuchsia.foo.Bar` from parent");

        let offer_decl: OfferDecl = OfferBuilder::protocol()
            .name("fuchsia.foo.Bar")
            .source(OfferSource::Self_)
            .target_static_child("child")
            .build();
        let seg: RouteSegment = crate::capability::RouteSegment::OfferBy {
            moniker: moniker.clone(),
            capability: offer_decl,
        }
        .into();
        assert_eq!(
            seg,
            RouteSegment::OfferBy {
                moniker: moniker.clone(),
                capability: "fuchsia.foo.Bar".to_string(),
                source: "self".to_string(),
                target: "child `#child`".to_string(),
            }
        );
        assert_eq!(
            format!("{seg}"),
            "`foo/bar` offered `fuchsia.foo.Bar` from self to child `#child`"
        );

        let expose_decl: ExposeDecl = ExposeBuilder::protocol()
            .name("fuchsia.foo.Bar")
            .source(ExposeSource::Self_)
            .target(ExposeTarget::Parent)
            .build();
        let seg: RouteSegment = crate::capability::RouteSegment::ExposeBy {
            moniker: moniker.clone(),
            capability: expose_decl,
        }
        .into();
        assert_eq!(
            seg,
            RouteSegment::ExposeBy {
                moniker: moniker.clone(),
                capability: "fuchsia.foo.Bar".to_string(),
                source: "self".to_string(),
                target: "parent".to_string(),
            }
        );
        assert_eq!(format!("{seg}"), "`foo/bar` exposed `fuchsia.foo.Bar` from self to parent");

        let cap_decl = CapabilityDecl::Protocol(ProtocolDecl {
            name: Name::new("fuchsia.foo.Bar").unwrap(),
            source_path: None,
            delivery: DeliveryType::Immediate,
        });
        let seg: RouteSegment = crate::capability::RouteSegment::DeclareBy {
            moniker: moniker.clone(),
            capability: cap_decl,
        }
        .into();
        assert_eq!(
            seg,
            RouteSegment::DeclareBy {
                moniker: moniker.clone(),
                capability: "fuchsia.foo.Bar".to_string(),
            }
        );
        assert_eq!(format!("{seg}"), "`foo/bar` declared capability `fuchsia.foo.Bar`");
    }

    #[cfg(feature = "serde")]
    #[test]
    fn test_route_segment_serde() {
        let moniker = Moniker::parse_str("foo/bar").unwrap();
        let seg = RouteSegment::UseBy {
            moniker,
            capability: "fuchsia.foo.Bar".to_string(),
            source: "parent".to_string(),
        };
        let serialized = serde_json::to_string(&seg).unwrap();
        assert_eq!(
            serialized,
            r#"{"type":"use_by","moniker":"foo/bar","capability":"fuchsia.foo.Bar","source":"parent"}"#
        );
        let deserialized: RouteSegment = serde_json::from_str(&serialized).unwrap();
        assert_eq!(seg, deserialized);
    }
}
