// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::Error;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::types::document::{DocumentContext, parse_and_hydrate};

pub trait FileResolver {
    /// Returns the absolute path found and the file content
    fn resolve(&self, path: &Path, current_dir: &Path) -> Result<(PathBuf, String), Error>;
}

pub struct OsResolver {
    include_paths: Vec<PathBuf>,
    include_root: PathBuf,
}

impl OsResolver {
    pub fn new(include_paths: Vec<PathBuf>, include_root: PathBuf) -> Self {
        let normalized_paths = include_paths.into_iter().map(|p| normalize_path(&p)).collect();
        let normalized_root = normalize_path(&include_root);

        Self { include_paths: normalized_paths, include_root: normalized_root }
    }
}

impl FileResolver for OsResolver {
    fn resolve(&self, path: &Path, current_dir: &Path) -> Result<(PathBuf, String), Error> {
        let path_str = path.to_str().unwrap_or("");

        let clean_path = if path_str.starts_with("//") {
            let p = self.include_root.join(&path_str[2..]);
            let norm = normalize_path(&p);
            if norm.exists() { Some(norm) } else { None }
        } else if path.is_absolute() {
            let norm = normalize_path(path);
            if norm.exists() { Some(norm) } else { None }
        } else {
            std::iter::once(current_dir.join(path))
                .chain(std::iter::once(path.to_path_buf()))
                .chain(self.include_paths.iter().map(|dir| dir.join(path)))
                .map(|p| normalize_path(&p))
                .find(|p| p.exists())
        };

        let clean_path =
            clean_path.ok_or_else(|| Error::Internal(format!("File not found: {:?}", path)))?;

        let content = std::fs::read_to_string(&clean_path)
            .map_err(|e| Error::Internal(format!("Read error {:?}: {}", clean_path, e)))?;

        Ok((clean_path, content))
    }
}

/// The invoker of CmlLoader is responsible for writing their own depfile.
pub struct CmlLoader<R: FileResolver> {
    resolver: R,
    visited: HashSet<PathBuf>,
}

impl<R: FileResolver> CmlLoader<R> {
    pub fn new(resolver: R) -> Self {
        Self { resolver, visited: HashSet::new() }
    }

    pub fn load_and_merge_all(&mut self, root_path: &Path) -> Result<DocumentContext, Error> {
        let (root_path_rel, buffer) = self.resolver.resolve(root_path, Path::new(""))?;

        self.visited.insert(root_path_rel.clone());
        let file_arc: Arc<std::path::Path> = Arc::from(root_path_rel.as_path());
        let mut root_doc = parse_and_hydrate(file_arc, &buffer)?;

        let mut stack = HashSet::new();
        stack.insert(root_path_rel.clone());

        self.resolve_includes_recursive(&mut root_doc, &root_path_rel, &mut stack)?;

        Ok(root_doc)
    }

    fn resolve_includes_recursive(
        &mut self,
        target_doc: &mut DocumentContext,
        current_file_path: &Path,
        stack: &mut HashSet<PathBuf>,
    ) -> Result<(), Error> {
        let current_dir = current_file_path.parent().unwrap_or_else(|| Path::new(""));

        if let Some(includes) = target_doc.include.take() {
            for include_span in includes {
                let include_path = Path::new(&include_span.value);

                let (shard_path_abs, buffer) = self
                    .resolver
                    .resolve(include_path, &current_dir)
                    .map_err(|e| e.with_origin(include_span.origin.clone()))?;

                if stack.contains(&shard_path_abs) {
                    return Err(Error::validate_context(
                        format!("Circular include detected: {:?}", shard_path_abs),
                        Some(include_span.origin.clone()),
                    ));
                }

                if self.visited.contains(&shard_path_abs) {
                    continue;
                }

                self.visited.insert(shard_path_abs.clone());
                stack.insert(shard_path_abs.clone());

                let file_arc: Arc<std::path::Path> = Arc::from(shard_path_abs.as_path());
                let mut shard_doc = parse_and_hydrate(file_arc, &buffer)
                    .map_err(|e| e.with_origin(include_span.origin.clone()))?;

                let result =
                    self.resolve_includes_recursive(&mut shard_doc, &shard_path_abs, stack);

                stack.remove(&shard_path_abs);

                result?;

                target_doc.merge_from(shard_doc, &shard_path_abs)?;
            }
        }
        Ok(())
    }

    pub fn visited_files(&self) -> HashSet<PathBuf> {
        self.visited.clone()
    }
}

/// Needed for hermetic builds so that the path is relative.
fn normalize_path(path: &Path) -> PathBuf {
    let mut ret = PathBuf::new();

    for component in path.components() {
        match component {
            std::path::Component::CurDir => {} // Ignore "."
            std::path::Component::ParentDir => {
                match ret.components().next_back() {
                    Some(std::path::Component::Normal(_)) => {
                        ret.pop();
                    }
                    Some(std::path::Component::RootDir) | Some(std::path::Component::Prefix(_)) => {
                        /* Do nothing */
                    }
                    _ => {
                        ret.push(component);
                    }
                }
            }
            _ => {
                // Push RootDir, Prefix, and Normal components
                ret.push(component);
            }
        }
    }
    ret
}

#[cfg(test)]
mod tests {
    use crate::OneOrMany;

    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    pub struct MockResolver {
        pub files: HashMap<PathBuf, String>,
    }

    impl FileResolver for MockResolver {
        fn resolve(&self, path: &Path, current_dir: &Path) -> Result<(PathBuf, String), Error> {
            let relative = current_dir.join(path);
            if let Some(content) = self.files.get(&relative) {
                return Ok((relative, content.clone()));
            }

            if let Some(content) = self.files.get(path) {
                return Ok((path.to_path_buf(), content.clone()));
            }

            Err(Error::internal(format!(
                "Mock file not found: {:?}. (Current dir: {:?})",
                path, current_dir
            )))
        }
    }

    #[test]
    fn test_include_empty_array() {
        let mut files = HashMap::new();
        let main_path = PathBuf::from("main.cml");

        files.insert(
            main_path.clone(),
            r#"{ "include": [], "program": { "runner": "elf" } }"#.to_string(),
        );

        let resolver = MockResolver { files };
        let mut loader = CmlLoader::new(resolver);

        let doc =
            loader.load_and_merge_all(&main_path).expect("Failed to handle empty include array");
        assert!(doc.program.is_some());
    }

    #[test]
    fn test_no_includes() {
        let mut files = HashMap::new();
        let main_path = PathBuf::from("main.cml");

        files.insert(main_path.clone(), r#"{ "program": { "runner": "elf" } }"#.to_string());

        let resolver = MockResolver { files };
        let mut loader = CmlLoader::new(resolver);

        let doc = loader
            .load_and_merge_all(&main_path)
            .expect("Failed to load document without includes");
        assert!(doc.program.is_some());
    }

    #[test]
    fn test_recursive_merge() {
        let mut files = HashMap::new();

        let root_path = PathBuf::from("/app/main.cml");
        let shard_path = PathBuf::from("/app/shards/network.shard.cml");

        files.insert(
            root_path.clone(),
            r#"{ "include": [ "shards/network.shard.cml" ] }"#.to_string(),
        );
        files.insert(
            shard_path.clone(),
            r#"{ "capabilities": [ { "protocol": "fuchsia.test.Protocol" } ] }"#.to_string(),
        );

        let resolver = MockResolver { files };
        let mut loader = CmlLoader::new(resolver);

        let doc = loader.load_and_merge_all(&root_path).expect("Mock load failed");

        let caps = doc.capabilities.as_ref().unwrap();
        let protocol_field = caps[0].value.protocol.as_ref().expect("Protocol missing");

        match &protocol_field.value {
            OneOrMany::One(name) => {
                assert_eq!(name.as_str(), "fuchsia.test.Protocol");
            }
            OneOrMany::Many(_) => {
                panic!("Expected a single protocol, found a list");
            }
        }

        assert_eq!(caps[0].origin.as_ref(), &shard_path);
    }

    #[test]
    fn test_invalid_include_file_not_found() {
        let mut files = HashMap::new();
        let main_path = PathBuf::from("main.cml");

        files.insert(main_path.clone(), r#"{ "include": ["doesnt_exist.cml"] }"#.to_string());

        let resolver = MockResolver { files };
        let mut loader = CmlLoader::new(resolver);

        let result = loader.load_and_merge_all(&main_path);

        assert!(result.is_err(), "Loader should fail when an include is missing");
    }

    #[test]
    fn test_relative_include_chain() {
        let mut files = HashMap::new();

        let root_path = PathBuf::from("/root.cml");
        let driver_path = PathBuf::from("/sys/driver.cml");
        let logger_path = PathBuf::from("/sys/logger.cml");

        files.insert(root_path.clone(), r#"{ "include": [ "sys/driver.cml" ] }"#.to_string());
        // driver.cml includes "logger.cml" relative to itself (/sys)
        files.insert(driver_path.clone(), r#"{ "include": [ "logger.cml" ] }"#.to_string());
        files.insert(
            logger_path.clone(),
            r#"{
            "capabilities": [ { "protocol": "fuchsia.sys.Logger" } ]
        }"#
            .to_string(),
        );

        let resolver = MockResolver { files };
        let mut loader = CmlLoader::new(resolver);

        let doc = loader.load_and_merge_all(&root_path).expect("Failed to follow relative chain");

        let caps = doc.capabilities.as_ref().unwrap();
        assert_eq!(caps.len(), 1);

        assert_eq!(caps[0].origin.as_ref(), &logger_path);
    }

    #[test]
    fn test_circular_include_error() {
        let mut files = HashMap::new();
        let a_path = PathBuf::from("/a.cml");
        let b_path = PathBuf::from("/b.cml");

        files.insert(a_path.clone(), r#"{ "include": [ "b.cml" ] }"#.to_string());
        files.insert(b_path.clone(), r#"{ "include": [ "a.cml" ] }"#.to_string());

        let resolver = MockResolver { files };
        let mut loader = CmlLoader::new(resolver);

        let result = loader.load_and_merge_all(&a_path);

        assert!(result.is_err());
    }

    #[test]
    fn test_include_from_search_path() {
        let mut files = HashMap::new();
        let root_path = PathBuf::from("/app/root.cml");

        let sdk_path = PathBuf::from("/sdk/lib/common.cml");

        files.insert(root_path.clone(), r#"{ "include": [ "common.cml" ] }"#.to_string());
        files.insert(sdk_path.clone(), r#"{ "offer": [] }"#.to_string());

        struct SearchPathMock {
            files: HashMap<PathBuf, String>,
        }
        impl FileResolver for SearchPathMock {
            fn resolve(&self, path: &Path, current_dir: &Path) -> Result<(PathBuf, String), Error> {
                let local = current_dir.join(path);
                if self.files.contains_key(&local.clone()) {
                    return Ok((local.clone(), self.files[&local].clone()));
                }

                let sdk_candidate = PathBuf::from("/sdk/lib").join(path);
                if let Some(content) = self.files.get(&sdk_candidate) {
                    return Ok((sdk_candidate, content.clone()));
                }
                Err(Error::internal("Not found"))
            }
        }

        let resolver = SearchPathMock { files };
        let mut loader = CmlLoader::new(resolver);

        let result = loader.load_and_merge_all(&root_path);
        assert!(result.is_ok(), "Loader should have found common.cml in the SDK path");
    }

    #[test]
    fn test_include_cml_with_dictionary() {
        let mut files = HashMap::new();
        let shard_path = PathBuf::from("shard.cml");
        let main_path = PathBuf::from("main.cml");

        files.insert(
            shard_path.clone(),
            json!({
                "expose": [
                    {
                        "dictionary": "diagnostics",
                        "from": "self",
                    }
                ],
                "capabilities": [
                    {
                        "dictionary": "diagnostics",
                    }
                ]
            })
            .to_string(),
        );
        files.insert(
            main_path.clone(),
            json!({
                "include": ["shard.cml"],
                "program": {
                    "binary": "bin/hello_world",
                    "runner": "foo"
                }
            })
            .to_string(),
        );

        let resolver = MockResolver { files };

        let mut loader = CmlLoader::new(resolver);
        let merged_doc = loader.load_and_merge_all(&main_path).unwrap();

        let expected_cml = json!({
            "program": {
                "binary": "bin/hello_world",
                "runner": "foo"
            },
            "expose": [
                {
                    "dictionary": "diagnostics",
                    "from": "self",
                }
            ],
            "capabilities": [
                {
                    "dictionary": "diagnostics",
                }
            ]
        })
        .to_string();

        let expected_doc = crate::load_cml_with_context(&expected_cml, Path::new("expected.cml"))
            .expect("failed to parse expected");

        assert_eq!(merged_doc, expected_doc)
    }

    #[test]
    fn test_diamond_dependency_is_safe() {
        let mut files = HashMap::new();

        let a = PathBuf::from("/a.cml");
        let b = PathBuf::from("/b.cml");
        let c = PathBuf::from("/c.cml");
        let d = PathBuf::from("/d.cml");

        files.insert(a.clone(), r#"{ "include": ["b.cml", "c.cml"] }"#.to_string());
        files.insert(b.clone(), r#"{ "include": ["d.cml"] }"#.to_string());
        files.insert(c.clone(), r#"{ "include": ["d.cml"] }"#.to_string());

        files.insert(
            d.clone(),
            r#"{
            "capabilities": [ { "protocol": "fuchsia.diamond.Protocol" } ]
        }"#
            .to_string(),
        );

        let resolver = MockResolver { files };
        let mut loader = CmlLoader::new(resolver);

        let doc = loader.load_and_merge_all(&a).expect("Diamond dependency should succeed");

        let caps = doc.capabilities.as_ref().unwrap();

        assert_eq!(caps.len(), 1, "Should have exactly one capability after merge");

        assert!(loader.visited_files().contains(&d));
    }

    #[test]
    fn test_include_multiple_shards() {
        let mut files = HashMap::new();
        let main_path = PathBuf::from("main.cml");

        files.insert(
            main_path.clone(),
            r#"{ "include": ["shard1.cml", "shard2.cml"] }"#.to_string(),
        );
        files.insert(
            PathBuf::from("shard1.cml"),
            r#"{ "use": [{ "protocol": "fuchsia.foo.A" }] }"#.to_string(),
        );
        files.insert(
            PathBuf::from("shard2.cml"),
            r#"{ "use": [{ "protocol": "fuchsia.foo.B" }] }"#.to_string(),
        );

        let resolver = MockResolver { files };
        let mut loader = CmlLoader::new(resolver);

        let doc = loader.load_and_merge_all(&main_path).expect("Failed to load multiple shards");

        let uses = doc.r#use.expect("Should have merged use block");
        assert_eq!(uses.len(), 2, "Should have loaded and merged both shards");
    }

    #[test]
    fn test_include_absolute_path() {
        let mut files = HashMap::new();
        let main_path = PathBuf::from("/some/nested/dir/main.cml");

        files.insert(main_path.clone(), r#"{ "include": ["//path/to/shard.cml"] }"#.to_string());

        // The mock file exists at the absolute root, not relative to main.cml
        files.insert(
            PathBuf::from("/path/to/shard.cml"),
            r#"{ "capabilities": [{ "protocol": "foo" }] }"#.to_string(),
        );

        let resolver = MockResolver { files };
        let mut loader = CmlLoader::new(resolver);

        let doc =
            loader.load_and_merge_all(&main_path).expect("Failed to resolve absolute include");
        assert!(doc.capabilities.is_some());
    }

    #[test]
    fn test_normalize_path() {
        assert_eq!(normalize_path(Path::new("a/b/../c")), PathBuf::from("a/c"));

        assert_eq!(
            normalize_path(Path::new("/fuchsia/out/default/../../sdk/lib")),
            PathBuf::from("/fuchsia/sdk/lib")
        );

        assert_eq!(normalize_path(Path::new("./a/./b/./c/.")), PathBuf::from("a/b/c"));

        assert_eq!(normalize_path(Path::new("/../../foo")), PathBuf::from("/foo"));

        let double_slash = normalize_path(Path::new("//sdk/lib/shard.cml"));
        assert!(double_slash.to_str().unwrap().contains("sdk/lib/shard.cml"));

        assert_eq!(normalize_path(Path::new("/a/b/c/../../d/./e/../f")), PathBuf::from("/a/d/f"));
    }

    #[test]
    fn test_normalize_relative_leading_dots() {
        assert_eq!(
            normalize_path(Path::new("../../examples/hello.cml")),
            PathBuf::from("../../examples/hello.cml")
        );

        assert_eq!(
            normalize_path(Path::new("../../sdk/lib/../inspect/client.shard.cml")),
            PathBuf::from("../../sdk/inspect/client.shard.cml")
        );
    }
}
