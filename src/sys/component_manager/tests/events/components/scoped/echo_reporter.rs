// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::format_err;
use component_events::events::*;
use component_events::matcher::*;
use fuchsia_component::client::connect_to_protocol;
use futures::StreamExt;
use {fidl_fidl_examples_routing_echo as fecho, fidl_fuchsia_component as fcomponent};

#[fuchsia::main]
async fn main() {
    // Connect to the event stream _before_ the server is started.
    let mut event_stream = EventStream::open().await.unwrap();

    // Cause the server component to start and stop by connecting to it and calling its echo
    // function.
    let echo = connect_to_protocol::<fecho::EchoMarker>().expect("error connecting to echo");
    let out = echo.echo_string(Some("Hippos rule!")).await.expect("echo_string failed");
    let out = out.ok_or_else(|| format_err!("empty result")).expect("echo_string got empty result");
    assert_eq!(out, "Hippos rule!");

    loop {
        let event = event_stream.next().await.unwrap();
        if matches!(event.header.unwrap().event_type.unwrap(), fcomponent::EventType::Started) {
            break;
        }
    }
    EventMatcher::ok()
        .stop(Some(ExitStatusMatcher::Clean))
        .moniker("./echo_server")
        .wait::<Stopped>(&mut event_stream)
        .await
        .unwrap();
}
