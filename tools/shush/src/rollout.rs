// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use anyhow::Result;

use crate::api::Api;
use crate::issues::Issue;

pub fn rollout(api: &mut (impl Api + ?Sized), rollout_path: &Path, verbose: bool) -> Result<()> {
    let created_issues =
        serde_json::from_reader::<_, Vec<Issue>>(BufReader::new(File::open(rollout_path)?))?;

    println!("Rolling out {} lints...", created_issues.len());

    let comment = "The toolchain has been updated at ToT, and this issue can now be \
    reproduced and fixed. If files here should not be owned by this component/owners, please add \
    or update the relevant OWNERS file and re-assign this bug."
        .to_owned();

    Issue::rollout(created_issues, Some(comment), api, verbose)?;

    Ok(())
}
