// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::cli::format::{format_action_error, format_resolve_error, format_start_error};
use crate::lifecycle::{resolve_instance, start_instance, unresolve_instance};
use crate::query::get_cml_moniker_from_query;
use anyhow::Result;
use flex_fuchsia_sys2 as fsys;

pub async fn reload_cmd<W: std::io::Write>(
    query: String,
    lifecycle_controller: fsys::LifecycleControllerProxy,
    realm_query: fsys::RealmQueryProxy,
    mut writer: W,
) -> Result<()> {
    let moniker = get_cml_moniker_from_query(&query, &realm_query).await?;

    writeln!(writer, "Moniker: {}", moniker)?;
    writeln!(writer, "Unresolving component instance...")?;

    unresolve_instance(&lifecycle_controller, &moniker)
        .await
        .map_err(|e| format_action_error(&moniker, e))?;

    writeln!(writer, "Resolving component instance...")?;

    resolve_instance(&lifecycle_controller, &moniker)
        .await
        .map_err(|e| format_resolve_error(&moniker, e))?;

    writeln!(writer, "Starting component instance...")?;

    let _ = start_instance(&lifecycle_controller, &moniker)
        .await
        .map_err(|e| format_start_error(&moniker, e))?;

    writeln!(writer, "Reloaded component instance!")?;
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::test_utils::{serve_lifecycle_controller, serve_realm_query_instances};

    #[fuchsia::test]
    async fn test_success() -> Result<()> {
        let mut output = Vec::new();
        let lifecycle_controller = serve_lifecycle_controller("/core/ffx-laboratory:test");
        let realm_query = serve_realm_query_instances(vec![fsys::Instance {
            moniker: Some("/core/ffx-laboratory:test".to_string()),
            url: Some("fuchsia-pkg://fuchsia.com/test#meta/test.cml".to_string()),
            instance_id: None,
            resolved_info: None,
            ..Default::default()
        }]);
        let response =
            reload_cmd("test".to_string(), lifecycle_controller, realm_query, &mut output).await;
        response.unwrap();
        Ok(())
    }
}
