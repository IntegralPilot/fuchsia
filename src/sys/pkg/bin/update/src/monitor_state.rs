// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_update::{MonitorRequest, MonitorRequestStream};
use fidl_fuchsia_update_ext::State;
use futures::prelude::*;
use std::io::Write;

fn print_state(state: &State) {
    if termion::is_tty(&std::io::stdout()) {
        print!("\r{state}\x1b[K");
        if state.is_terminal() {
            println!();
        }
    } else {
        println!("State: {state:?}")
    }
    std::io::stdout().flush().unwrap();
}

pub async fn monitor_state(mut stream: MonitorRequestStream) -> Result<(), anyhow::Error> {
    while let Some(event) = stream.try_next().await? {
        match event {
            MonitorRequest::OnState { state, responder } => {
                responder.send()?;

                let state = State::from(state);

                // Exit if we encounter an error during an update.
                if state.is_error() {
                    anyhow::bail!("Update failed: {:?}", state)
                } else {
                    print_state(&state);
                }
            }
        }
    }
    Ok(())
}
