// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use fho::{FfxContext, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use {
    fdomain_fuchsia_developer_remotecontrol as rc,
    fdomain_fuchsia_starnix_container as fstarcontainer,
};

use crate::common::*;

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "vmo",
    example = "ffx starnix vmo -k 123456",
    description = "Return all processes that have references to a file backed by a vmo with koid"
)]
pub struct StarnixVmoCommand {
    #[argh(option, short = 'k')]
    /// koid of the vmo to search for references to.
    pub koid: u64,
}

pub async fn starnix_vmo(
    StarnixVmoCommand { koid }: StarnixVmoCommand,
    rcs_proxy: &rc::RemoteControlProxy,
) -> Result<VmoCommandOutput> {
    let controller_proxy = connect_to_controller(&rcs_proxy, None).await?;
    let mut references = controller_proxy
        .get_vmo_references(&fstarcontainer::ControllerGetVmoReferencesRequest {
            koid: Some(koid),
            ..Default::default()
        })
        .await
        .bug_context("fetching vmo references")?
        .references
        .bug_context("reading references field")?
        .into_iter()
        .map(VmoReference::try_from)
        .collect::<Result<Vec<_>, _>>()
        .bug_context("converting fidl type to json type")?;
    references.sort();
    Ok(VmoCommandOutput { koid, references })
}

#[derive(Debug, JsonSchema, Deserialize, Serialize)]
pub struct VmoCommandOutput {
    pub koid: u64,
    pub references: Vec<VmoReference>,
}

impl std::fmt::Display for VmoCommandOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.references.is_empty() {
            writeln!(f, "Didn't find any references to vmo with koid {:?}", self.koid)?;
        } else {
            for reference in &self.references {
                writeln!(f, "{reference}")?;
            }
        }
        Ok(())
    }
}

// Redefinition of fidl_fuchsia_starnix_container_common::VmoReference for JSON.
#[derive(Debug, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
pub struct VmoReference {
    /// The Starnix pid of the process containing a file backed by the vmo.
    pub pid: u64,
    /// The name of the process containing a file backed by the vmo.
    pub process_name: String,
    /// The file descriptor number in the process that refers to the vmo.
    pub fd: i32,
    /// The koid of the vmo.
    pub koid: u64,
}

impl std::fmt::Display for VmoReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "pid: {}, name: {}, fd: {}", self.pid, self.process_name, self.fd)
    }
}

impl TryFrom<fstarcontainer::VmoReference> for VmoReference {
    type Error = anyhow::Error;

    fn try_from(value: fstarcontainer::VmoReference) -> std::result::Result<Self, Self::Error> {
        Ok(Self {
            pid: value.pid.bug_context("reading pid field")?,
            process_name: value.process_name.bug_context("reading process_name field")?,
            fd: value.fd.bug_context("reading fd field")?,
            koid: value.koid.bug_context("reading koid field")?,
        })
    }
}
