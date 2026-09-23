// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::query::get_cml_moniker_from_query;
use crate::route::{self, RouteReport};
use anyhow::{format_err, Result};
use flex_fuchsia_sys2 as fsys;

pub async fn route_cmd_print<W: std::io::Write>(
    target_moniker: String,
    filter: Option<String>,
    route_validator: fsys::RouteValidatorProxy,
    realm_query: fsys::RealmQueryProxy,
    mut writer: W,
) -> Result<()> {
    let moniker = get_cml_moniker_from_query(&target_moniker, &realm_query).await?;

    writeln!(writer, "Moniker: {}", moniker)?;

    let targets = route_targets_from_filter(filter)?;
    let reports = route::route(&route_validator, moniker, targets).await?;

    let table = route::create_table(reports);
    table.print(&mut writer)?;
    writeln!(writer, "")?;

    Ok(())
}

pub async fn route_cmd_serialized(
    target_moniker: String,
    filter: Option<String>,
    route_validator: fsys::RouteValidatorProxy,
    realm_query: fsys::RealmQueryProxy,
) -> Result<Vec<RouteReport>> {
    let moniker = get_cml_moniker_from_query(&target_moniker, &realm_query).await?;
    let targets = route_targets_from_filter(filter)?;
    route::route(&route_validator, moniker, targets).await
}

fn route_targets_from_filter(filter: Option<String>) -> Result<Vec<fsys::RouteTarget>> {
    if filter.is_none() {
        return Ok(vec![]);
    }
    let filter = filter.unwrap();
    filter
        .split(',')
        .into_iter()
        .map(|entry| {
            let parts: Vec<_> = entry.split(':').collect();
            match parts.len() {
                2 => {
                    let decl_type = match parts[0] {
                        "use" => fsys::DeclType::Use,
                        "expose" => fsys::DeclType::Expose,
                        _ => {
                            return Err(invalid_filter_err(&entry));
                        }
                    };
                    Ok(fsys::RouteTarget { name: parts[1].into(), decl_type })
                }
                1 => {
                    Ok(fsys::RouteTarget { name: parts[0].into(), decl_type: fsys::DeclType::Any })
                }
                _ => Err(invalid_filter_err(&entry)),
            }
        })
        .collect()
}

fn invalid_filter_err(filter: &str) -> anyhow::Error {
    format_err!(
        "Invalid filter: {}. Format must match <capability>, \
        use:<capability>, or expose:<capability>.",
        filter
    )
}
