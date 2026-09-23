// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(feature = "fdomain")]
use component_debug_fdomain as component_debug;

use crate::commands::types::*;
use crate::commands::utils;
use crate::types::Error;
use argh::{ArgsInfo, FromArgs};
use component_debug::realm::Instance;
use diagnostics_data::InspectData;
use flex_fuchsia_diagnostics::Selector;
use flex_fuchsia_sys2 as fsys;
use serde::{Serialize, Serializer};
use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fmt;

#[derive(Debug, Eq, PartialEq, PartialOrd, Ord, Serialize)]
pub struct MonikerWithUrl {
    pub moniker: String,
    pub component_url: String,
}

#[derive(Debug, Eq, PartialEq)]
pub enum ListResultItem {
    Moniker(String),
    MonikerWithUrl(MonikerWithUrl),
}

impl ListResultItem {
    pub fn into_moniker(self) -> String {
        let moniker = match self {
            Self::Moniker(moniker) => moniker,
            Self::MonikerWithUrl(MonikerWithUrl { moniker, .. }) => moniker,
        };
        selectors::sanitize_moniker_for_selectors(&moniker)
    }
}

impl Ord for ListResultItem {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (ListResultItem::Moniker(moniker), ListResultItem::Moniker(other_moniker))
            | (
                ListResultItem::MonikerWithUrl(MonikerWithUrl { moniker, .. }),
                ListResultItem::MonikerWithUrl(MonikerWithUrl { moniker: other_moniker, .. }),
            ) => moniker.cmp(other_moniker),
            _ => unreachable!("all lists must contain variants of the same type"),
        }
    }
}

impl PartialOrd for ListResultItem {
    // Compare based on the moniker only. To enable sorting using the moniker only.
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Serialize for ListResultItem {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Moniker(string) => serializer.serialize_str(string),
            Self::MonikerWithUrl(data) => data.serialize(serializer),
        }
    }
}

#[derive(Serialize)]
pub struct ListResult(Vec<ListResultItem>);

impl fmt::Display for ListResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for item in self.0.iter() {
            match item {
                ListResultItem::Moniker(moniker) => writeln!(f, "{moniker}")?,
                ListResultItem::MonikerWithUrl(MonikerWithUrl { component_url, moniker }) => {
                    writeln!(f, "{moniker}:")?;
                    writeln!(f, "  {component_url}")?;
                }
            }
        }
        Ok(())
    }
}

fn components_from_inspect_data(
    inspect_data: Vec<InspectData>,
) -> impl Iterator<Item = ListResultItem> {
    inspect_data.into_iter().map(|value| {
        ListResultItem::MonikerWithUrl(MonikerWithUrl {
            moniker: value.moniker.to_string(),
            component_url: value.metadata.component_url.into(),
        })
    })
}

pub fn list_response_items(
    with_url: bool,
    components: impl Iterator<Item = ListResultItem>,
) -> Vec<ListResultItem> {
    components
        .map(|result| {
            if with_url {
                result
            } else {
                match result {
                    ListResultItem::Moniker(_) => result,
                    ListResultItem::MonikerWithUrl(val) => ListResultItem::Moniker(val.moniker),
                }
            }
        })
        // Collect as btreeset to sort and remove potential duplicates.
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
}

/// Lists all components (relative to the scope where the archivist receives events from) of
/// components that expose inspect.
#[derive(Default, ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(subcommand, name = "list")]
pub struct ListCommand {
    #[argh(option)]
    /// a fuzzy-search query. May include URL, moniker, or manifest fragments.
    /// a fauzzy-search query for the component we are interested in. May include URL, moniker, or
    /// manifest fragments. If this is provided, the output will only contain monikers for
    /// components that matched the query.
    pub component: Option<String>,

    #[argh(switch)]
    /// also print the URL of the component.
    pub with_url: bool,

    #[argh(option)]
    /// a selector specifying what `fuchsia.diagnostics.ArchiveAccessor` to connect to.
    /// The selector will be in the form of:
    /// <moniker>:fuchsia.diagnostics.ArchiveAccessor(.pipeline_name)?
    ///
    /// Typically this is the output of `iquery list-accessors`.
    ///
    /// For example: `bootstrap/archivist:fuchsia.diagnostics.ArchiveAccessor.feedback`
    /// means that the command will connect to the `ArchiveAccecssor` filtered by the feedback
    /// pipeline exposed by `bootstrap/archivist`.
    pub accessor: Option<String>,
}

impl Command for ListCommand {
    type Result = ListResult;

    async fn execute<P: DiagnosticsProvider>(self, provider: &P) -> Result<Self::Result, Error> {
        let mut selectors = if let Some(query) = self.component {
            let instances = find_components(provider.realm_query(), &query).await?;
            instances_to_root_selectors(instances)?
        } else {
            vec![]
        };
        utils::ensure_tree_field_is_set(&mut selectors, None)?;
        let inspect = provider.snapshot(self.accessor.as_deref(), selectors).await?;
        let components = components_from_inspect_data(inspect);
        let results = list_response_items(self.with_url, components);
        Ok(ListResult(results))
    }
}

async fn find_components(
    realm_proxy: &fsys::RealmQueryProxy,
    query: &str,
) -> Result<Vec<Instance>, Error> {
    component_debug::query::get_instances_from_query(query, realm_proxy)
        .await
        .map_err(Error::FuzzyMatchRealmQuery)
}

fn instances_to_root_selectors(instances: Vec<Instance>) -> Result<Vec<Selector>, Error> {
    instances
        .into_iter()
        .map(|instance| instance.moniker)
        .map(|moniker| {
            let selector_str = format!("{moniker}:root");
            selectors::parse_verbose(&selector_str).map_err(Error::PartialSelectorHint)
        })
        .collect::<Result<Vec<Selector>, Error>>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_data::{InspectDataBuilder, InspectHandleName, Timestamp};

    #[fuchsia::test]
    fn components_from_inspect_data_uses_diagnostics_ready() {
        let inspect_data = vec![
            InspectDataBuilder::new(
                "some_moniker".try_into().unwrap(),
                "fake-url",
                Timestamp::from_nanos(123456789800i64),
            )
            .with_name(InspectHandleName::filename("fake-file"))
            .build(),
            InspectDataBuilder::new(
                "other_moniker".try_into().unwrap(),
                "other-fake-url",
                Timestamp::from_nanos(123456789900i64),
            )
            .with_name(InspectHandleName::filename("fake-file"))
            .build(),
            InspectDataBuilder::new(
                "some_moniker".try_into().unwrap(),
                "fake-url",
                Timestamp::from_nanos(123456789910i64),
            )
            .with_name(InspectHandleName::filename("fake-file"))
            .build(),
            InspectDataBuilder::new(
                "different_moniker".try_into().unwrap(),
                "different-fake-url",
                Timestamp::from_nanos(123456790990i64),
            )
            .with_name(InspectHandleName::filename("fake-file"))
            .build(),
        ];

        let components = components_from_inspect_data(inspect_data).collect::<Vec<_>>();

        assert_eq!(components.len(), 4);
    }
}
