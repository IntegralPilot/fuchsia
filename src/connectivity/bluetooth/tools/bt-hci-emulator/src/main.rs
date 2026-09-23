// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error};
use fuchsia_bluetooth::constants::DEV_DIR;
use futures::future::pending;
use hci_emulator_client::Emulator;

fn usage(appname: &str) {
    eprintln!("usage: {}", appname);
    eprintln!("       {} --help", appname);
    eprintln!("");
    eprintln!("Instantiate and manipulate a new bt-hci device emulator");
    eprintln!(
        "examples: {}                - Instantiates a new emulator device with a random ID",
        appname
    );
}

// TODO(armansito): Add ways to pass controller settings.
#[fuchsia::main(logging = false)]
async fn main() -> Result<(), Error> {
    let args: Vec<_> = std::env::args().collect();
    match args.as_slice() {
        [] => unreachable!(),
        [_] => {}
        [appname, ..] => {
            usage(appname);
            return Ok(());
        }
    };

    let dev_dir = fuchsia_fs::directory::open_in_namespace(DEV_DIR, fuchsia_fs::PERM_READABLE)
        .with_context(|| format!("failed to open {}", DEV_DIR))?;

    let emulator = Emulator::create_and_publish(dev_dir).await?;

    let topo_path = emulator.get_topological_path().await?;
    eprintln!("Instantiated emulator at path: {}", topo_path);

    // TODO(armansito): Instantiate a REPL here. For now we await forever to make sure that the
    // emulator device remains alive until the user terminates this program (it will be removed when
    // `emulator` drops).
    pending().await
}
