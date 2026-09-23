// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::anyhow;
use futures::future::FutureExt as _;

#[fuchsia::main(logging = false)]
async fn main() {
    match main_impl().await {
        Ok(()) => (),
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    }
}

async fn main_impl() -> anyhow::Result<()> {
    let first_arg =
        &std::env::args_os().next().ok_or_else(|| anyhow!("binary called without any args"))?;
    match AsRef::<std::path::Path>::as_ref(first_arg)
        .file_name()
        .ok_or_else(|| anyhow!("first argument has no filename: {first_arg:?}"))?
        .to_str()
        .ok_or_else(|| anyhow!("first argument is not unicode: {first_arg:?}"))?
    {
        "component" => component::exec().boxed_local().await,
        "package" => package::exec().boxed_local().await,
        other => Err(anyhow!("no program registered for: {other}")),
    }
}
