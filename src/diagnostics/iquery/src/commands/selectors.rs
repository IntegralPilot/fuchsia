// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::commands::types::*;
use crate::commands::utils;
use crate::types::Error;
use argh::{ArgsInfo, FromArgs};
use diagnostics_data::{InspectData, InspectHandleName};
use diagnostics_hierarchy::DiagnosticsHierarchy;
use flex_fuchsia_diagnostics as fdiagnostics;
use serde::ser::{Error as _, SerializeSeq};
use serde::{Serialize, Serializer};
use std::fmt;

/// Lists all available selectors for the given input of component queries or partial selectors.
#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(subcommand, name = "selectors")]
pub struct SelectorsCommand {
    #[argh(positional)]
    /// component query, component selector, or component and tree selector. Minimum: 1 unless
    /// `--component` is set. When `--component` is provided then the selectors should be tree
    /// selectors, otherwise they can be component selectors or component and tree selectors.
    /// Full selectors (including a property segment) are allowed but not informative.
    pub selectors: Vec<String>,

    #[argh(option)]
    /// tree selectors to splice onto a component query specified as a positional argument
    ///
    /// For example, `show foo.cm --data root:bar` becomes the selector `path/to/foo:root:bar`.
    pub data: Vec<String>,

    #[argh(option)]
    /// a string specifying what `fuchsia.diagnostics.ArchiveAccessor` to connect to.
    /// This can be copied from the output of `ffx inspect list-accessors`.
    /// The selector will be in the form of:
    /// <moniker>:fuchsia.diagnostics.ArchiveAccessor.pipeline_name
    pub accessor: Option<String>,
}

impl Command for SelectorsCommand {
    type Result = SelectorsResult;

    async fn execute<P: DiagnosticsProvider>(self, provider: &P) -> Result<Self::Result, Error> {
        if self.selectors.is_empty() && self.data.is_empty() {
            return Err(Error::invalid_arguments("Expected 1 or more selectors. Got zero."));
        }

        let mut selectors = if self.data.is_empty() {
            utils::process_fuzzy_inputs(self.selectors, provider).await?
        } else {
            if self.selectors.len() != 1 {
                return Err(Error::WrongNumberOfSearchQueriesForDataFlag);
            }
            utils::process_component_query_with_partial_selectors(
                &self.selectors[0],
                self.data.into_iter(),
                provider,
            )
            .await?
        };

        utils::ensure_tree_field_is_set(&mut selectors, None)?;
        let mut results = provider.snapshot(self.accessor.as_deref(), selectors).await?;
        for result in results.iter_mut() {
            if let Some(hierarchy) = &mut result.payload {
                hierarchy.sort();
            }
        }
        Ok(SelectorsResult(inspect_to_selectors(results)))
    }
}

pub struct SelectorsResult(Vec<fdiagnostics::Selector>);

impl Serialize for SelectorsResult {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        let mut stringified = self
            .0
            .iter()
            .map(|item| {
                selectors::selector_to_string(
                    item,
                    selectors::SelectorDisplayOptions::never_wrap_in_quotes(),
                )
                .map_err(|e| S::Error::custom(format!("failed to serialize: {e:#?}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        stringified.sort();
        for item in stringified {
            seq.serialize_element(&item)?;
        }

        seq.end()
    }
}

impl fmt::Display for SelectorsResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut stringified = self
            .0
            .iter()
            .map(|item| {
                selectors::selector_to_string(item, selectors::SelectorDisplayOptions::default())
                    .map_err(|_| fmt::Error)
            })
            .collect::<Result<Vec<_>, _>>()?;
        stringified.sort();
        for item in stringified {
            writeln!(f, "{item}")?;
        }
        Ok(())
    }
}

fn get_selectors(
    moniker: String,
    hierarchy: DiagnosticsHierarchy,
    name: InspectHandleName,
) -> Vec<fdiagnostics::Selector> {
    hierarchy
        .property_iter()
        .flat_map(|(node_path, maybe_property)| maybe_property.map(|prop| (node_path, prop)))
        .map(|(node_path, property)| {
            let node_path = node_path
                .iter()
                .map(|s| fdiagnostics::StringSelector::ExactMatch(s.to_string()))
                .collect::<Vec<_>>();

            let target_properties =
                fdiagnostics::StringSelector::ExactMatch(property.name().to_string());

            let tree_selector = Some(fdiagnostics::TreeSelector::PropertySelector(
                fdiagnostics::PropertySelector { node_path, target_properties },
            ));

            let tree_names = Some(fdiagnostics::TreeNames::Some(vec![name.to_string()]));

            let component_selector = Some(fdiagnostics::ComponentSelector {
                moniker_segments: Some(
                    moniker
                        .split("/")
                        .map(|segment| {
                            fdiagnostics::StringSelector::ExactMatch(segment.to_string())
                        })
                        .collect(),
                ),
                ..Default::default()
            });

            fdiagnostics::Selector {
                component_selector,
                tree_selector,
                tree_names,
                ..Default::default()
            }
        })
        .collect()
}

fn inspect_to_selectors(inspect_data: Vec<InspectData>) -> Vec<fdiagnostics::Selector> {
    inspect_data
        .into_iter()
        .filter_map(|schema| {
            let moniker = schema.moniker;
            let name = schema.metadata.name;
            schema.payload.map(|hierarchy| get_selectors(moniker.to_string(), hierarchy, name))
        })
        .flatten()
        .collect()
}
