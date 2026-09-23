// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use diagnostics_reader::ArchiveReader;
use fidl_fuchsia_data as fdata;
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, Ref, Route};
use futures::StreamExt;
use regex::Regex;
use std::fs::File;
use std::io::{self, BufRead};

#[fuchsia::test]
async fn wisdom_integration_test() -> Result<(), Error> {
    // Create the test realm.
    let builder = RealmBuilder::new().await?;
    let wisdom_server =
        builder.add_child("wisdom_server", "#meta/wisdom_server.cm", ChildOptions::new()).await?;
    let wisdom_client = builder
        .add_child("wisdom_client", "#meta/wisdom_client.cm", ChildOptions::new().eager())
        .await?;
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name(
                    "fuchsia.examples.intl.wisdom.IntlWisdomServer",
                ))
                .from(&wisdom_server)
                .to(&wisdom_client),
        )
        .await?;
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                .from(Ref::parent())
                .to(&wisdom_server)
                .to(&wisdom_client),
        )
        .await?;

    // Inject the program.args of the manifest
    let mut client_decl = builder.get_component_decl(&wisdom_client).await?;
    let program = client_decl.program.as_mut().unwrap();
    program.info.entries.as_mut().unwrap().push(fdata::DictionaryEntry {
        key: "args".into(),
        value: Some(Box::new(fdata::DictionaryValue::StrVec(vec![
            "--timestamp=2018-11-01T12:34:56Z".to_string(),
            "--timezone=America/Los_Angeles".to_string(),
        ]))),
    });
    builder.replace_component_decl(&wisdom_client, client_decl).await?;

    // Create the realm instance
    let realm_instance = builder.build().await?;

    // Initialize the log reader
    let moniker = format!("realm_builder\\:{}/wisdom_client", realm_instance.root.child_name());
    let mut reader = ArchiveReader::logs();
    reader.select_all_for_component(format!("{}", moniker).as_str());
    let mut log_stream = reader.snapshot_then_subscribe()?;

    // Initialize Goldens file
    let goldens = File::open("/pkg/data/golden-output.txt")?;
    let mut lines = io::BufReader::new(goldens).lines();

    // TODO(https://fxbug.dev/42071702): There are flaky tests that caused by unexpected
    // blank lines on log. It maybe an issue on the stdout to log forwarded,
    // we may want to change to use fidl to pass the result from client to test
    // body to avoid this issue.

    // Verify each line matches the log output
    while let Some(line_regex) = lines.next() {
        let logs = log_stream.next().await.expect("got log result")?;
        let log_msg = logs.msg().unwrap();
        let line_regex_str = line_regex.expect("line_regex");
        let regex = Regex::new(&line_regex_str).expect("regex");
        assert!(
            regex.is_match(log_msg),
            "\n\tline_regex: {:?}\n\tactual    : {:?}",
            &line_regex_str,
            &log_msg
        );
    }

    // Clean up the realm instance
    realm_instance.destroy().await?;

    Ok(())
}
