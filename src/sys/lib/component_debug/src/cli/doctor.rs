// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::doctor::{create_tables, validate_routes, RouteReport};
use crate::query::get_cml_moniker_from_query;
use anyhow::Result;
use moniker::Moniker;

use flex_fuchsia_sys2 as fsys;

pub async fn doctor_cmd_print<W: std::io::Write>(
    query: String,
    route_validator: fsys::RouteValidatorProxy,
    realm_query: fsys::RealmQueryProxy,
    mut writer: W,
) -> Result<()> {
    let moniker = get_cml_moniker_from_query(&query, &realm_query).await?;
    writeln!(writer, "Moniker: {}", moniker)?;

    let reports = validate_routes(&route_validator, &moniker).await?;
    write_result_table(&moniker, &reports, writer)?;
    Ok(())
}

pub fn write_result_table<W: std::io::Write>(
    moniker: &Moniker,
    reports: &Vec<RouteReport>,
    mut writer: W,
) -> Result<()> {
    let (use_table, expose_table) = create_tables(reports);
    use_table.print(&mut writer)?;
    writeln!(writer, "")?;

    expose_table.print(&mut writer)?;
    writeln!(writer, "")?;

    let mut error_capabilities =
        reports.iter().filter(|r| r.error_summary.is_some()).map(|r| &r.capability).peekable();
    if error_capabilities.peek().is_some() {
        writeln!(writer, "For further diagnosis, try:\n")?;
        for cap in error_capabilities {
            writeln!(writer, "  $ ffx component route {} {}", moniker, cap)?;
        }
    }
    Ok(())
}

pub async fn doctor_cmd_serialized(
    query: String,
    route_validator: fsys::RouteValidatorProxy,
    realm_query: fsys::RealmQueryProxy,
) -> Result<Vec<RouteReport>> {
    let moniker = get_cml_moniker_from_query(&query, &realm_query).await?;
    validate_routes(&route_validator, &moniker).await
}
