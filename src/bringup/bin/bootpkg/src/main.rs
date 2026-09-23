// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use fidl_fuchsia_io as fio;

fn main() -> anyhow::Result<()> {
    let args: bootpkg::args::Args = argh::from_env();
    let boot_dir = fdio::open_fd("/boot", fio::PERM_READABLE | fio::Flags::PROTOCOL_DIRECTORY)?;
    bootpkg::bootpkg(boot_dir, args)
}
