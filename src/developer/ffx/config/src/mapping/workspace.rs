// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::path::Path;

pub(crate) fn find_workspace_root(mut current: &Path) -> Option<&Path> {
    loop {
        if current.join("WORKSPACE").exists() || current.join("WORKSPACE.bazel").exists() {
            return Some(current);
        } else {
            current = current.parent()?;
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
// tests
#[cfg(test)]
mod test {
    use super::*;
    use std::fs::File;
    use tempfile::{TempDir, tempdir};

    fn make_temp_workspace_dir(workspace_filename: Option<&str>) -> TempDir {
        let workspace = tempdir().expect("temp directory");
        if let Some(name) = workspace_filename {
            File::create(workspace.path().join(name)).expect("workspace file");
        }
        workspace
    }

    #[test]
    fn test_find_workspace() {
        for filename in ["WORKSPACE", "WORKSPACE.bazel"] {
            let workspace_dir = make_temp_workspace_dir(Some(filename));
            assert_eq!(
                None,
                find_workspace_root(workspace_dir.path().parent().unwrap()),
                "Shouldn't find workspace in outer directory"
            );
            assert_eq!(
                Some(workspace_dir.path()),
                find_workspace_root(workspace_dir.path()),
                "Find workspace in the root"
            );

            std::fs::create_dir_all(workspace_dir.path().join("first/second/third")).unwrap();
            for subdir in ["first", "first/second", "first/second/third"] {
                let subdir = workspace_dir.path().join(subdir);
                assert_eq!(
                    Some(workspace_dir.path()),
                    find_workspace_root(&subdir),
                    "Find workspace in a subdirectory"
                );
            }
        }
    }
}
