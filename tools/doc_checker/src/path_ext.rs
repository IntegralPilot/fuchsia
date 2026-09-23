// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

/// Extension methods for `std::path::Path` specific to Fuchsia
/// documentation checking.
pub trait DocPathExt {
    /// Returns true if this path lies within developer-facing tools
    /// (e.g., under 'skills').
    fn is_ignored_doc(&self) -> bool;

    /// Returns true if this path represents a doc navbar.
    fn is_navbar_doc(&self) -> bool;

    /// Returns true if the file is a macOS metadata file (starts with '._').
    fn is_macos_hidden_doc(&self) -> bool;

    /// Returns true if this path has hidden/private components (starts with
    /// '_') relative to the documentation roots.
    ///
    /// Strips `root_dir` and `reference_docs_root` prefixes before performing
    /// the check, to prevent false-positive ignore matches if the checkout
    /// directory itself contains an underscore.
    fn is_hidden_doc(&self, root_dir: &Path, reference_docs_root: Option<&Path>) -> bool;
}

impl DocPathExt for Path {
    fn is_ignored_doc(&self) -> bool {
        self.components()
            .any(|c| matches!(c, Component::Normal(name) if name == OsStr::new("skills")))
    }

    fn is_navbar_doc(&self) -> bool {
        self.file_name() == Some(OsStr::new("navbar.md"))
    }

    fn is_macos_hidden_doc(&self) -> bool {
        self.file_name().and_then(|name| name.to_str()).map_or(false, |s| s.starts_with("._"))
    }

    fn is_hidden_doc(&self, root_dir: &Path, reference_docs_root: Option<&Path>) -> bool {
        let rel_p = self
            .strip_prefix(root_dir)
            .ok()
            .or_else(|| reference_docs_root.and_then(|r| self.strip_prefix(r).ok()))
            .or_else(|| if self.is_relative() { Some(self) } else { None });

        rel_p.map_or(false, |p| {
            p.components().any(|c| {
                matches!(c, Component::Normal(s) if s.to_str().unwrap_or_default().starts_with('_'))
            })
        })
    }
}

/// Standard path normalization that resolves '.' and '..' components without accessing the filesystem.
/// Returns `Err` if the path escapes the root (i.e. starts with '..').
pub fn normalize_path(path: &Path) -> anyhow::Result<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(p) => normalized.push(p.as_os_str()),
            Component::RootDir => normalized.push("/"),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    anyhow::bail!(
                        "Cannot normalize {}, references parent beyond root.",
                        path.display()
                    );
                }
            }
            Component::Normal(p) => normalized.push(p),
        }
    }
    Ok(normalized)
}

/// Normalizes the path and verifies it remains within the specified `root_dir`.
/// If the normalized path is outside `root_dir`, returns an error.
pub fn normalize_and_validate_path(path: &Path, root_dir: &Path) -> anyhow::Result<PathBuf> {
    let normalized_root = normalize_path(root_dir).unwrap_or_else(|_| root_dir.to_path_buf());
    if let Ok(normalized_path) = normalize_path(path) {
        if normalized_path.starts_with(&normalized_root) {
            return Ok(normalized_path);
        }
    }

    let normalized = normalize_path(&root_dir.join(path))?;
    if normalized.starts_with(&normalized_root) {
        Ok(normalized)
    } else {
        anyhow::bail!("Included markdown file {:?} escapes workspace root.", path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_is_ignored_doc() {
        assert!(Path::new("docs/skills/SKILL.md").is_ignored_doc());
        assert!(Path::new("vendor/google/skills/yaml/config.yaml").is_ignored_doc());
        assert!(!Path::new("docs/contribute/governance.md").is_ignored_doc());
        assert!(!Path::new("docs/_toc.yaml").is_ignored_doc());
    }

    #[test]
    fn test_is_navbar_doc() {
        assert!(Path::new("docs/navbar.md").is_navbar_doc());
        assert!(Path::new("navbar.md").is_navbar_doc());
        assert!(!Path::new("docs/README.md").is_navbar_doc());
    }

    #[test]
    fn test_is_macos_hidden_doc() {
        assert!(Path::new("docs/._README.md").is_macos_hidden_doc());
        assert!(Path::new("._index.md").is_macos_hidden_doc());
        assert!(!Path::new("docs/README.md").is_macos_hidden_doc());
    }

    #[test]
    fn test_is_hidden_doc() {
        let root_dir = PathBuf::from("/home/user/fuchsia");
        let ref_dir = PathBuf::from("/home/user/reference_docs");

        // Standard file (should not be hidden)
        let p1 = PathBuf::from("/home/user/fuchsia/docs/getting-started.md");
        assert!(!p1.is_hidden_doc(&root_dir, None));

        // File starting with underscore (should be hidden)
        let p2 = PathBuf::from("/home/user/fuchsia/docs/_index.md");
        assert!(p2.is_hidden_doc(&root_dir, None));

        // File inside a hidden folder (should be hidden)
        let p3 = PathBuf::from("/home/user/fuchsia/docs/_common/header.md");
        assert!(p3.is_hidden_doc(&root_dir, None));

        // Standard file in reference docs (should not be hidden)
        let p4 = PathBuf::from("/home/user/reference_docs/sdk/overview.md");
        assert!(!p4.is_hidden_doc(&root_dir, Some(&ref_dir)));

        // File inside a hidden folder in reference docs (should be hidden)
        let p5 = PathBuf::from("/home/user/reference_docs/_internal/helper.md");
        assert!(p5.is_hidden_doc(&root_dir, Some(&ref_dir)));

        // Relative paths (when self is already relative to workspace root)
        let p6 = PathBuf::from("docs/_common/header.md");
        assert!(p6.is_hidden_doc(&root_dir, None));

        let p7 = PathBuf::from("_index.md");
        assert!(p7.is_hidden_doc(&root_dir, None));

        let p8 = PathBuf::from("docs/getting-started.md");
        assert!(!p8.is_hidden_doc(&root_dir, None));

        // Deeply nested hidden directory
        let p9 = PathBuf::from("/home/user/fuchsia/docs/guides/_internal/advanced/details.md");
        assert!(p9.is_hidden_doc(&root_dir, None));
    }

    #[test]
    fn test_is_hidden_doc_with_underscore_in_workspace_roots() {
        // Scenario where workspace path contains underscore (e.g., /home/user/_workspace)
        let root_dir = PathBuf::from("/home/user/_workspace/fuchsia");
        let ref_dir = PathBuf::from("/home/user/_workspace/reference");

        // Standard file should NOT be hidden, even though workspace path has underscore
        let p1 = PathBuf::from("/home/user/_workspace/fuchsia/docs/getting-started.md");
        assert!(!p1.is_hidden_doc(&root_dir, Some(&ref_dir)));

        let p2 = PathBuf::from("/home/user/_workspace/reference/sdk/overview.md");
        assert!(!p2.is_hidden_doc(&root_dir, Some(&ref_dir)));

        // Hidden file inside workspace with underscore should still be detected correctly
        let p3 = PathBuf::from("/home/user/_workspace/fuchsia/docs/_common/header.md");
        assert!(p3.is_hidden_doc(&root_dir, Some(&ref_dir)));

        let p4 = PathBuf::from("/home/user/_workspace/reference/_internal/helper.md");
        assert!(p4.is_hidden_doc(&root_dir, Some(&ref_dir)));
    }

    #[test]
    fn test_normalize_path() {
        assert_eq!(normalize_path(Path::new("a/b/../c")).unwrap(), PathBuf::from("a/c"));
        assert_eq!(normalize_path(Path::new("./a/b/.")).unwrap(), PathBuf::from("a/b"));
        assert_eq!(normalize_path(Path::new("/a/b/c")).unwrap(), PathBuf::from("/a/b/c"));
        assert!(normalize_path(Path::new("a/../../b")).is_err());
    }

    #[test]
    fn test_normalize_and_validate_path() {
        let root_dir = PathBuf::from("/home/user/fuchsia");

        // Valid relative path
        assert_eq!(
            normalize_and_validate_path(Path::new("docs/getting-started.md"), &root_dir).unwrap(),
            PathBuf::from("/home/user/fuchsia/docs/getting-started.md")
        );

        // Valid absolute path
        assert_eq!(
            normalize_and_validate_path(
                Path::new("/home/user/fuchsia/docs/getting-started.md"),
                &root_dir
            )
            .unwrap(),
            PathBuf::from("/home/user/fuchsia/docs/getting-started.md")
        );

        // Escaping root path
        assert!(normalize_and_validate_path(Path::new("../external/file.md"), &root_dir).is_err());
    }
}
