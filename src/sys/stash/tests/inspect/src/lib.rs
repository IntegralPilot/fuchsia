// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use diagnostics_assertions::assert_data_tree;
use diagnostics_reader::ArchiveReader;

#[fuchsia::test]
async fn stash_inspect() -> Result<(), Error> {
    // stash is started as an eager child of the test's top-level component.
    let data = ArchiveReader::inspect().add_selector("stash:root").snapshot().await?;
    assert_eq!(1, data.len());

    assert_data_tree!(data[0].payload.as_ref().unwrap(),
        root: contains {
            secure_mode: false,
            "fuchsia.inspect.Health": contains {
                status: "OK"
            }
        }
    );
    Ok(())
}

#[fuchsia::test]
async fn stash_secure_inspect() -> Result<(), Error> {
    // stash_secure is started as an eager child of the test's top-level component.
    let data = ArchiveReader::inspect().add_selector("stash_secure:root").snapshot().await?;
    assert_eq!(1, data.len());

    assert_data_tree!(data[0].payload.as_ref().unwrap(),
        root: contains {
            secure_mode: true,
            "fuchsia.inspect.Health": contains {
                status: "OK"
            }
        }
    );

    Ok(())
}
