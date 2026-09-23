// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::FromArgs;
use camino::Utf8PathBuf;

use anyhow::Result;
use depfile::Depfile;

use starnix_container::StarnixContainerGenerator;

/// Construct a starnix container that can include an Android system and HALs.
#[derive(FromArgs)]
struct Command {
    /// name of the starnix container.
    #[argh(option)]
    name: String,

    /// directory to place outputs into.
    #[argh(option)]
    outdir: Utf8PathBuf,

    /// path to package archive containing additional resources to include.
    #[argh(option)]
    base: Utf8PathBuf,

    /// path to an Android system image.
    #[argh(option)]
    system: Utf8PathBuf,

    /// path to an Android vendor partition image.
    #[argh(option)]
    vendor: Option<Utf8PathBuf>,

    /// path to the ramdisk images (which will be concatenated).
    #[argh(option)]
    ramdisk: Vec<Utf8PathBuf>,

    /// path to hal package archive.
    #[argh(option)]
    hal: Vec<Utf8PathBuf>,

    /// path to a depfile to write.
    #[argh(option)]
    depfile: Option<Utf8PathBuf>,

    /// path to fstab, will go in /odm which overrides the one in /vendor
    #[argh(option)]
    fstab: Option<Utf8PathBuf>,

    /// path to extra init scripts, will go in /odm/etc/init. Can be passed more than once.
    #[argh(option)]
    init: Vec<Utf8PathBuf>,

    /// whether to skip including HALs as subpackages.
    #[argh(switch)]
    skip_subpackages: bool,
}

fn main() -> Result<()> {
    let cmd: Command = argh::from_env();
    let container = StarnixContainerGenerator {
        name: cmd.name,
        outdir: cmd.outdir,
        base: cmd.base,
        hals: cmd.hal,
        skip_subpackages: cmd.skip_subpackages,
        system: cmd.system,
        vendor: cmd.vendor,
        ramdisk: cmd.ramdisk,
        fstab: cmd.fstab,
        init: cmd.init,
        file_overrides: vec![],
    };

    let mut depfile = Depfile::new();
    container.build(&mut depfile)?;

    if let Some(path) = cmd.depfile {
        depfile.write_to(path)?;
    }

    Ok(())
}
