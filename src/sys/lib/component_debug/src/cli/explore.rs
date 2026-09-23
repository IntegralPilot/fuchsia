// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::explore::*;
use crate::query::get_cml_moniker_from_query;
use anyhow::{Result, anyhow};
use flex_client::ProxyHasDomain;
use flex_fuchsia_dash as fdash;
use flex_fuchsia_data as fdata;
use flex_fuchsia_sys2 as fsys;

pub async fn explore_cmd(
    query: String,
    ns_layout: DashNamespaceLayout,
    command: Option<String>,
    overridden_tools_urls: Vec<String>,
    dash_launcher: fdash::LauncherProxy,
    realm_query: fsys::RealmQueryProxy,
    stdout: socket_to_stdio::Stdout<'_>,
) -> Result<()> {
    let moniker = get_cml_moniker_from_query(&query, &realm_query).await?;
    println!("Moniker: {}\r", moniker);

    let tool_urls = if overridden_tools_urls.len() != 0 {
        overridden_tools_urls
    } else {
        // We haven't overridden the tool_urls on the command line, so check the component manifest
        // to see if it has default tool_urls
        let urls = get_tool_urls_from_component_manifest(&realm_query, &moniker)
            .await
            .unwrap_or_else(|e| {
                println!("Error loading tool urls from component manifest: {e:?}");
                vec![]
            });
        println!("Using tool URLs from component manifest: {urls:?}\r");
        urls
    };

    let (client, server) = realm_query.domain().create_stream_socket();

    explore_over_socket(moniker, server, tool_urls, command, ns_layout, &dash_launcher).await?;

    #[cfg(not(feature = "fdomain"))]
    socket_to_stdio::connect_socket_to_stdio(client, stdout).await?;

    #[cfg(feature = "fdomain")]
    socket_to_stdio::connect_fdomain_socket_to_stdio(client, stdout).await?;

    let exit_code = wait_for_shell_exit(&dash_launcher).await?;

    std::process::exit(exit_code);
}

async fn get_tool_urls_from_component_manifest(
    query: &fsys::RealmQueryProxy,
    moniker: &moniker::Moniker,
) -> Result<Vec<String>> {
    let component_manifest = crate::realm::get_resolved_declaration(moniker, query)
        .await
        .map_err(|e| anyhow!("Couldn't get manifest for component {moniker}: {e:?}"))?;
    let Some(facets) = component_manifest.facets else {
        return Ok(vec![]);
    };

    urls_from_facets(facets)
}

fn urls_from_facets(facets: fdata::Dictionary) -> Result<Vec<String>> {
    let mut urls = vec![];
    for facet in facets.entries.as_ref().unwrap_or(&vec![]) {
        if !facet.key.eq("fuchsia.dash.launcher-tool-urls") {
            continue;
        }
        let Some(val) = facet.value.clone() else {
            continue;
        };

        match *val {
            fdata::DictionaryValue::Str(tool) => {
                urls.push(tool);
            }
            fdata::DictionaryValue::StrVec(tools) => {
                urls.extend(tools);
            }
            _ => {
                return Err(anyhow!(
                    "no parsable value for tool_urls facet. Override by passing --tools: {facet:?}"
                ));
            }
        }
    }
    Ok(urls)
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use flex_fuchsia_data as fdata;

    #[test]
    fn test_urls_from_facets_empty() {
        let facets = fdata::Dictionary { entries: Some(vec![]), ..Default::default() };
        let result = urls_from_facets(facets).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_urls_from_facets_no_matching_key() {
        let facets = fdata::Dictionary {
            entries: Some(vec![fdata::DictionaryEntry {
                key: "other.key".to_string(),
                value: Some(Box::new(fdata::DictionaryValue::Str("value".to_string()))),
            }]),
            ..Default::default()
        };
        let result = urls_from_facets(facets).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_urls_from_facets_single_url() {
        let facets = fdata::Dictionary {
            entries: Some(vec![fdata::DictionaryEntry {
                key: "fuchsia.dash.launcher-tool-urls".to_string(),
                value: Some(Box::new(fdata::DictionaryValue::Str("url1".to_string()))),
            }]),
            ..Default::default()
        };
        let result = urls_from_facets(facets).unwrap();
        assert_eq!(result, vec!["url1".to_string()]);
    }

    #[test]
    fn test_urls_from_facets_multiple_urls() {
        let facets = fdata::Dictionary {
            entries: Some(vec![fdata::DictionaryEntry {
                key: "fuchsia.dash.launcher-tool-urls".to_string(),
                value: Some(Box::new(fdata::DictionaryValue::StrVec(vec![
                    "url1".to_string(),
                    "url2".to_string(),
                ]))),
            }]),
            ..Default::default()
        };
        let result = urls_from_facets(facets).unwrap();
        assert_eq!(result, vec!["url1".to_string(), "url2".to_string()]);
    }

    #[test]
    fn test_urls_from_facets_invalid_value_type() {
        let facets = fdata::Dictionary {
            entries: Some(vec![fdata::DictionaryEntry {
                key: "fuchsia.dash.launcher-tool-urls".to_string(),
                value: Some(Box::new(fdata::DictionaryValue::ObjVec(vec![]))),
            }]),
            ..Default::default()
        };
        let result = urls_from_facets(facets);
        assert_matches!(result, Err(_));
    }

    #[test]
    fn test_urls_from_facets_mixed_entries() {
        let facets = fdata::Dictionary {
            entries: Some(vec![
                fdata::DictionaryEntry {
                    key: "other.key".to_string(),
                    value: Some(Box::new(fdata::DictionaryValue::Str("value".to_string()))),
                },
                fdata::DictionaryEntry {
                    key: "fuchsia.dash.launcher-tool-urls".to_string(),
                    value: Some(Box::new(fdata::DictionaryValue::Str("url1".to_string()))),
                },
            ]),
            ..Default::default()
        };
        let result = urls_from_facets(facets).unwrap();
        assert_eq!(result, vec!["url1".to_string()]);
    }

    #[test]
    fn test_urls_from_facets_none_value() {
        let facets = fdata::Dictionary {
            entries: Some(vec![fdata::DictionaryEntry {
                key: "fuchsia.dash.launcher-tool-urls".to_string(),
                value: None,
            }]),
            ..Default::default()
        };
        let result = urls_from_facets(facets).unwrap();
        assert!(result.is_empty());
    }
}
