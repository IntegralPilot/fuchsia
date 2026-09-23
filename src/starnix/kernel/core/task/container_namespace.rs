// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use fidl::endpoints::create_endpoints;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use {fidl_fuchsia_component_runner as frunner, fidl_fuchsia_unknown as funknown};

#[derive(Debug)]
pub struct ContainerNamespace {
    /// Internal collection of path to namespace entry.
    namespace_entries: HashMap<PathBuf, funknown::CloneableSynchronousProxy>,
}

impl ContainerNamespace {
    pub fn new() -> Self {
        Self { namespace_entries: Default::default() }
    }

    /// Returns a bool indicating whether this ContainerNamespace contains a
    /// channel entry corresponding to the given path.
    pub fn has_channel_entry(&self, channel_path: impl AsRef<Path>) -> bool {
        return self.namespace_entries.contains_key(channel_path.as_ref());
    }

    /// Attempts to find a channel at the given namespace path.
    pub fn get_namespace_channel(
        &self,
        channel_path: impl AsRef<Path>,
    ) -> Result<zx::Channel, anyhow::Error> {
        let path = channel_path.as_ref();
        if !path.is_absolute() {
            anyhow::bail!(
                "Invalid parameter provided to get_namespace_channel: {}",
                path.display()
            );
        }

        match self.namespace_entries.get(path) {
            Some(cloneable_proxy) => {
                let (cloned_client, cloned_server) =
                    create_endpoints::<funknown::CloneableMarker>();
                let clone_result = cloneable_proxy.clone(cloned_server);
                if clone_result.is_err() {
                    anyhow::bail!("Unable to clone the proxy channel for {}!", path.display())
                }
                Ok(cloned_client.into_channel())
            }
            None => anyhow::bail!("Could not find an entry for {}", path.display()),
        }
    }

    /// Iterate backwards through the path ancestors, until we find a channel
    /// which matches. This is needed since namespaces can be nested
    /// (e.g. /foo/bar and /foo), so we should find the closest match to our
    /// input query (e.g. /foo/bar/some should match /foo/bar). Returns the
    /// namespace proxy which was found, and the remaining subdirectory paths.
    /// For instance, if the input parameter is `/foo/bar/test` and we have a
    /// namespace corresponding to `/foo/bar`, then this function will return
    /// a proxy to `/foo/bar` and the remaining subdir `/test`.
    pub fn find_closest_channel(
        &self,
        search_path: impl AsRef<Path>,
    ) -> Result<(zx::Channel, String), anyhow::Error> {
        let search_path = search_path.as_ref();
        if !search_path.is_absolute() {
            anyhow::bail!(
                "Non-absolute path provided to find_closest_channel: {}",
                search_path.display()
            );
        }

        let mut root_channel = None;
        let mut remaining_subdir = String::new();

        let mut ns_path_ancestors = search_path.ancestors();
        while let Some(path) = ns_path_ancestors.next() {
            // If there is not an entry, we'll continue looking and prepend the
            // last path segment as a remaining subdir.
            if !self.has_channel_entry(path) {
                let last_segment = path
                    .components()
                    .next_back()
                    .and_then(|component| component.as_os_str().to_str())
                    .unwrap_or("");
                remaining_subdir.insert_str(0, &format!("{last_segment}/"));
                continue;
            }

            // If we found an entry, we can save and halt the search.
            root_channel = Some(self.get_namespace_channel(path)?);
            break;
        }

        if let Some(channel) = root_channel {
            // Trim the trailing `/` from the remaining subdirs.
            remaining_subdir.pop();
            Ok((channel, remaining_subdir))
        } else {
            anyhow::bail!("Unable to find a namespace corresponding to {}", search_path.display());
        }
    }

    /// Attempts to clone this ContainerNamespace, returning an error if the
    /// proxy cloning process failed.
    pub fn try_clone(&self) -> Result<ContainerNamespace, anyhow::Error> {
        let mut cloned_entries = HashMap::new();
        for (path, _) in &self.namespace_entries {
            match self.get_namespace_channel(path) {
                Ok(cloned_channel) => {
                    cloned_entries.insert(
                        path.clone(),
                        funknown::CloneableSynchronousProxy::new(cloned_channel),
                    );
                }
                Err(err) => {
                    anyhow::bail!(
                        "The ContainerNamespace clone operation for {} has failed: {}",
                        path.display(),
                        err,
                    )
                }
            }
        }
        Ok(ContainerNamespace { namespace_entries: cloned_entries })
    }
}

impl From<Vec<frunner::ComponentNamespaceEntry>> for ContainerNamespace {
    fn from(namespace: Vec<frunner::ComponentNamespaceEntry>) -> Self {
        let mut namespace_entries = HashMap::new();
        for mut entry in namespace {
            if let (Some(entry_name), Some(entry_dir)) =
                (entry.path.clone(), entry.directory.take())
            {
                let entry_channel = entry_dir.into_channel();
                namespace_entries.insert(
                    PathBuf::from(entry_name),
                    funknown::CloneableSynchronousProxy::new(entry_channel),
                );
            }
        }
        ContainerNamespace { namespace_entries }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use fidl::endpoints::{ClientEnd, Proxy};
    use fidl_fuchsia_io as fio;
    use fuchsia_fs::directory;

    #[::fuchsia::test]
    fn correctly_reports_entries() {
        // Initialize with only the /pkg channel.
        let _stub_exec = fuchsia_async::TestExecutor::new();
        let mut ns = Vec::<frunner::ComponentNamespaceEntry>::new();
        let pkg_channel: zx::Channel =
            directory::open_in_namespace("/pkg", fio::PERM_READABLE | fio::PERM_EXECUTABLE)
                .expect("failed to open /pkg")
                .into_channel()
                .expect("into_channel")
                .into();
        let data_handle = ClientEnd::new(pkg_channel);
        ns.push(frunner::ComponentNamespaceEntry {
            path: Some("/pkg".to_string()),
            directory: Some(data_handle),
            ..Default::default()
        });

        // Assert that /pkg is reported, and /data is not.
        let cn_under_test = ContainerNamespace::from(ns);
        assert!(cn_under_test.has_channel_entry("/pkg"));
        assert_eq!(cn_under_test.has_channel_entry("/data"), false);
    }

    #[::fuchsia::test]
    fn correctly_provides_and_retains_channel_entries() {
        // Initialize with only the /pkg channel.
        let _stub_exec = fuchsia_async::TestExecutor::new();
        let mut ns = Vec::<frunner::ComponentNamespaceEntry>::new();
        let pkg_channel: zx::Channel =
            directory::open_in_namespace("/pkg", fio::PERM_READABLE | fio::PERM_EXECUTABLE)
                .expect("failed to open /pkg")
                .into_channel()
                .expect("into_channel")
                .into();
        let data_handle = ClientEnd::new(pkg_channel);
        ns.push(frunner::ComponentNamespaceEntry {
            path: Some("/pkg".to_string()),
            directory: Some(data_handle),
            ..Default::default()
        });

        // Assert that we can get a channel for /pkg, that the channel is valid,
        // and that the ContainerNamespace still retains its own /pkg reference.
        let cn_under_test = ContainerNamespace::from(ns);
        let returned_channel = cn_under_test
            .get_namespace_channel("/pkg")
            .expect("get_namespace_channel should return a valid /pkg channel.");
        assert!(returned_channel.write(b"hello", &mut vec![]).is_ok());
        assert!(cn_under_test.has_channel_entry("/pkg"));
    }

    #[::fuchsia::test]
    fn returns_err_on_invalid_request() {
        let _stub_exec = fuchsia_async::TestExecutor::new();

        // Initialize with no channels, and validate request fails.
        let cn_under_test = ContainerNamespace::new();
        assert!(cn_under_test.get_namespace_channel("/pkg").is_err());
    }

    #[::fuchsia::test]
    fn correctly_returns_closest_channel_partial_match() {
        // Initialize with only the /pkg channel.
        let _stub_exec = fuchsia_async::TestExecutor::new();
        let mut ns = Vec::<frunner::ComponentNamespaceEntry>::new();
        let pkg_channel: zx::Channel =
            directory::open_in_namespace("/pkg", fio::PERM_READABLE | fio::PERM_EXECUTABLE)
                .expect("failed to open /pkg")
                .into_channel()
                .expect("into_channel")
                .into();
        let data_handle = ClientEnd::new(pkg_channel);
        ns.push(frunner::ComponentNamespaceEntry {
            path: Some("/pkg".to_string()),
            directory: Some(data_handle),
            ..Default::default()
        });

        // Assert that a request for an namespace extension channel
        // (e.g. /pkg/foo/test) returns a channel for the root (e.g. /pkg).
        let cn_under_test = ContainerNamespace::from(ns);
        let (returned_channel, subdir) = cn_under_test
            .find_closest_channel("/pkg/foo/bar")
            .expect("get_namespace_channel should return a valid /pkg channel.");

        // Assert that the channel is valid, and the ContainerNamespace retains
        // a reference to the root channel as well.
        assert!(returned_channel.write(b"hello", &mut vec![]).is_ok());
        assert!(cn_under_test.has_channel_entry("/pkg"));

        // Assert that the remaining subdir returned is correct.
        assert_eq!(subdir, "foo/bar");
    }

    #[::fuchsia::test]
    fn correctly_returns_closest_channel_exact_match() {
        // Initialize with only the /pkg channel.
        let _stub_exec = fuchsia_async::TestExecutor::new();
        let mut ns = Vec::<frunner::ComponentNamespaceEntry>::new();
        let pkg_channel: zx::Channel =
            directory::open_in_namespace("/pkg", fio::PERM_READABLE | fio::PERM_EXECUTABLE)
                .expect("failed to open /pkg")
                .into_channel()
                .expect("into_channel")
                .into();
        let data_handle = ClientEnd::new(pkg_channel);
        ns.push(frunner::ComponentNamespaceEntry {
            path: Some("/pkg".to_string()),
            directory: Some(data_handle),
            ..Default::default()
        });

        // Assert that a request for an exact namespace (e.g. /pkg)
        // returns a channel for the namespace (e.g. /pkg).
        let cn_under_test = ContainerNamespace::from(ns);
        let (returned_channel, subdir) = cn_under_test
            .find_closest_channel("/pkg")
            .expect("get_namespace_channel should return a valid /pkg channel.");

        // Assert that the channel is valid, and the ContainerNamespace retains
        // a reference to the root channel as well.
        assert!(returned_channel.write(b"hello", &mut vec![]).is_ok());
        assert!(cn_under_test.has_channel_entry("/pkg"));

        // Assert that the remaining subdir returned is correct.
        assert_eq!(subdir, "");
    }

    #[::fuchsia::test]
    fn correctly_clones() {
        // Initialize with only the /pkg channel.
        let _stub_exec = fuchsia_async::TestExecutor::new();
        let mut ns = Vec::<frunner::ComponentNamespaceEntry>::new();
        let pkg_channel: zx::Channel =
            directory::open_in_namespace("/pkg", fio::PERM_READABLE | fio::PERM_EXECUTABLE)
                .expect("failed to open /pkg")
                .into_channel()
                .expect("into_channel")
                .into();
        let data_handle = ClientEnd::new(pkg_channel);
        ns.push(frunner::ComponentNamespaceEntry {
            path: Some("/pkg".to_string()),
            directory: Some(data_handle),
            ..Default::default()
        });

        let cn_under_test = ContainerNamespace::from(ns);
        let clone_under_test = cn_under_test.try_clone().expect("Clone should succeed.");

        // Assert both original and clone contain /pkg channel references,
        // and that those channels are both valid.
        let original_channel = cn_under_test
            .get_namespace_channel("/pkg")
            .expect("get_namespace_channel should return a valid /pkg channel.");
        let cloned_channel = clone_under_test
            .get_namespace_channel("/pkg")
            .expect("get_namespace_channel should return a valid /pkg channel.");
        assert!(original_channel.write(b"hello", &mut vec![]).is_ok());
        assert!(cloned_channel.write(b"hello", &mut vec![]).is_ok());

        // Assert both original and clone retain their own references.
        assert!(cn_under_test.has_channel_entry("/pkg"));
        assert!(clone_under_test.has_channel_entry("/pkg"));
    }
}
