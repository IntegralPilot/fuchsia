// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::cli::format::format_create_error;
use crate::lifecycle::create_instance_in_collection;
use anyhow::{Result, format_err};
use flex_fuchsia_component_decl as fdecl;
use flex_fuchsia_sys2 as fsys;
use fuchsia_url::fuchsia_pkg::AbsoluteComponentUrl;
use moniker::Moniker;

pub async fn create_cmd<W: std::io::Write>(
    url: AbsoluteComponentUrl,
    moniker: Moniker,
    config_overrides: Vec<fdecl::ConfigOverride>,
    lifecycle_controller: fsys::LifecycleControllerProxy,
    mut writer: W,
) -> Result<()> {
    let parent = moniker
        .parent()
        .ok_or_else(|| format_err!("Error: {} does not reference a dynamic instance", moniker))?;
    let leaf = moniker
        .leaf()
        .ok_or_else(|| format_err!("Error: {} does not reference a dynamic instance", moniker))?;
    let child_name = leaf.name();
    let collection = leaf
        .collection()
        .ok_or_else(|| format_err!("Error: {} does not reference a dynamic instance", moniker))?;

    writeln!(writer, "URL: {}", url)?;
    writeln!(writer, "Moniker: {}", moniker)?;
    writeln!(writer, "Creating component instance...")?;

    create_instance_in_collection(
        &lifecycle_controller,
        &parent,
        collection,
        child_name,
        &url,
        config_overrides,
        None,
    )
    .await
    .map_err(|e| format_create_error(&moniker, &parent, collection, e))?;

    writeln!(writer, "Created component instance!")?;
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use fidl::endpoints::create_proxy_and_stream;
    use futures::TryStreamExt;

    fn setup_fake_lifecycle_controller(
        expected_moniker: &'static str,
        expected_collection: &'static str,
        expected_name: &'static str,
        expected_url: &'static str,
    ) -> fsys::LifecycleControllerProxy {
        let (lifecycle_controller, mut stream) =
            create_proxy_and_stream::<fsys::LifecycleControllerMarker>();
        fuchsia_async::Task::local(async move {
            let req = stream.try_next().await.unwrap().unwrap();
            match req {
                fsys::LifecycleControllerRequest::CreateInstance {
                    parent_moniker,
                    collection,
                    decl,
                    responder,
                    ..
                } => {
                    assert_eq!(
                        Moniker::parse_str(expected_moniker),
                        Moniker::parse_str(&parent_moniker)
                    );
                    assert_eq!(expected_collection, collection.name);
                    assert_eq!(expected_name, decl.name.unwrap());
                    assert_eq!(expected_url, decl.url.unwrap());
                    responder.send(Ok(())).unwrap();
                }
                _ => panic!("Unexpected Lifecycle Controller request"),
            }
        })
        .detach();
        lifecycle_controller
    }

    #[fuchsia::test]
    async fn test_success() -> Result<()> {
        let mut output = Vec::new();
        let lifecycle_controller = setup_fake_lifecycle_controller(
            "core",
            "ffx-laboratory",
            "test",
            "fuchsia-pkg://fuchsia.com/test#meta/test.cm",
        );
        let response = create_cmd(
            "fuchsia-pkg://fuchsia.com/test#meta/test.cm".try_into().unwrap(),
            "core/ffx-laboratory:test".try_into().unwrap(),
            vec![],
            lifecycle_controller,
            &mut output,
        )
        .await;
        response.unwrap();
        Ok(())
    }
}
