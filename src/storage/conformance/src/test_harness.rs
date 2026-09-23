// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::flags::Rights;
use fidl::endpoints::create_proxy;
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_io_test as io_test;

/// Helper struct for connecting to an io1 test harness and running a conformance test on it.
pub struct TestHarness {
    /// FIDL proxy to the io1 test harness.
    pub proxy: io_test::TestHarnessProxy,

    /// Config for the filesystem.
    pub config: io_test::HarnessConfig,

    /// All [`io_test::Directory`] rights supported by the filesystem.
    pub dir_rights: Rights,

    /// All [`io_test::File`] rights supported by the filesystem.
    pub file_rights: Rights,

    /// All [`io_test::ExecutableFile`] rights supported by the filesystem.
    pub executable_file_rights: Rights,

    /// All [`io_test::Symlink`] rights supported by the filesystem.
    pub symlink_rights: Rights,
}

impl TestHarness {
    /// Connects to the test harness and returns a `TestHarness` struct.
    pub async fn new() -> TestHarness {
        let proxy = connect_to_harness();
        let config = proxy.get_config().await.expect("Could not get config from proxy");

        // Validate configuration options for consistency, disallow invalid combinations.
        if config.supports_modify_directory {
            assert!(
                config.supports_get_token,
                "GetToken must be supported for testing Rename/Link!"
            );
        }
        if config.supports_append {
            assert!(config.supports_mutable_file, "Files supporting append must also be mutable.");
        }
        if config.supports_truncate {
            assert!(
                config.supports_mutable_file,
                "Files supporting truncate must also be mutable."
            );
        }

        // Generate set of supported open rights for each object type.
        let dir_rights = Rights::new(get_supported_dir_rights(&config));
        let file_rights = Rights::new(get_supported_file_rights(&config));
        let executable_file_rights = Rights::new(fio::Rights::READ_BYTES | fio::Rights::EXECUTE);
        let symlink_rights = Rights::new(get_supported_symlink_rights(&config));

        TestHarness {
            proxy,
            config,
            dir_rights,
            file_rights,
            executable_file_rights,
            symlink_rights,
        }
    }

    /// Creates a [`fio::DirectoryProxy`] with the given root directory structure.
    pub fn get_directory(
        &self,
        entries: Vec<io_test::DirectoryEntry>,
        flags: fio::Flags,
    ) -> fio::DirectoryProxy {
        let contents: Vec<Option<Box<io_test::DirectoryEntry>>> =
            entries.into_iter().map(|e| Some(Box::new(e))).collect();
        let (client, server) = create_proxy::<fio::DirectoryMarker>();
        self.proxy
            .create_directory(contents, flags, server)
            .expect("Cannot get directory from test harness");
        client
    }

    /// Helper function which gets service directory from the harness as a [`fio::DirectoryProxy`].
    /// Requires that the harness supports service directories, otherwise will panic.
    pub async fn open_service_directory(&self) -> fio::DirectoryProxy {
        assert!(self.config.supports_services);
        let client_end = self.proxy.open_service_directory().await.unwrap();
        client_end.into_proxy()
    }

    /// Returns the abilities [`io_test::File`] objects should have for the harness.
    pub fn supported_file_abilities(&self) -> fio::Abilities {
        let mut abilities = fio::Abilities::READ_BYTES | fio::Abilities::GET_ATTRIBUTES;
        if self.config.supports_mutable_file {
            abilities |= fio::Abilities::WRITE_BYTES;
        }
        if self.supports_mutable_attrs() {
            abilities |= fio::Abilities::UPDATE_ATTRIBUTES;
        }
        abilities
    }

    /// Returns the abilities [`io_test::Directory`] objects should have for the harness.
    pub fn supported_dir_abilities(&self) -> fio::Abilities {
        if self.config.supports_modify_directory {
            fio::Abilities::GET_ATTRIBUTES
                | fio::Abilities::UPDATE_ATTRIBUTES
                | fio::Abilities::ENUMERATE
                | fio::Abilities::TRAVERSE
                | fio::Abilities::MODIFY_DIRECTORY
        } else {
            fio::Abilities::GET_ATTRIBUTES | fio::Abilities::ENUMERATE | fio::Abilities::TRAVERSE
        }
    }

    /// Returns the abilities [`io_test::Symlink`] objects should have for the harness.
    pub fn supported_symlink_abilities(&self) -> fio::Abilities {
        let mut abilities = fio::Abilities::GET_ATTRIBUTES;
        if self.supports_mutable_attrs() {
            abilities |= fio::Abilities::UPDATE_ATTRIBUTES;
        }
        abilities
    }

    /// Returns true if the harness supports at least one mutable attribute, false otherwise.
    ///
    /// *NOTE*: To allow testing both the io1 SetAttrs and io2 UpdateAttributes methods, harnesses
    /// that support mutable attributes must support [`fio::NodeAttributesQuery::CREATION_TIME`]
    /// and [`fio::NodeAttributesQuery::MODIFICATION_TIME`].
    pub fn supports_mutable_attrs(&self) -> bool {
        supports_mutable_attrs(&self.config)
    }
}

fn connect_to_harness() -> io_test::TestHarnessProxy {
    // Connect to the realm to get access to the outgoing directory for the harness.
    let (client, server) = zx::Channel::create();
    fuchsia_component::client::connect_channel_to_protocol::<fcomponent::RealmMarker>(server)
        .expect("Cannot connect to Realm service");
    let realm = fcomponent::RealmSynchronousProxy::new(client);
    // fs_test is the name of the child component defined in the manifest.
    let child_ref = fdecl::ChildRef { name: "fs_test".to_string(), collection: None };
    let (client, server) = zx::Channel::create();
    realm
        .open_exposed_dir(
            &child_ref,
            fidl::endpoints::ServerEnd::<fio::DirectoryMarker>::new(server),
            zx::MonotonicInstant::INFINITE,
        )
        .expect("FIDL error when binding to child in Realm")
        .expect("Cannot bind to test harness child in Realm");

    let exposed_dir = fio::DirectoryProxy::new(fidl::AsyncChannel::from_channel(client));

    fuchsia_component::client::connect_to_protocol_at_dir_root::<io_test::TestHarnessMarker>(
        &exposed_dir,
    )
    .expect("Cannot connect to test harness protocol")
}

// Returns the aggregate of all rights that are supported for [`io_test::Directory`] objects.
// Note that rights are specific to a connection (abilities are properties of the node).
fn get_supported_dir_rights(config: &io_test::HarnessConfig) -> fio::Rights {
    fio::R_STAR_DIR
        | fio::W_STAR_DIR
        | if config.supports_executable_file { fio::X_STAR_DIR } else { fio::Rights::empty() }
}

// Returns the aggregate of all rights that are supported for [`io_test::File`] objects.
// Note that rights are specific to a connection (abilities are properties of the node).
fn get_supported_file_rights(config: &io_test::HarnessConfig) -> fio::Rights {
    let mut rights = fio::Rights::READ_BYTES | fio::Rights::GET_ATTRIBUTES;
    if config.supports_mutable_file {
        rights |= fio::Rights::WRITE_BYTES;
    }
    if supports_mutable_attrs(&config) {
        rights |= fio::Rights::WRITE_BYTES;
    }
    rights
}

// Returns the aggregate of all rights that are supported for [`io_test::Symlink`] objects.
// Note that rights are specific to a connection (abilities are properties of the node).
fn get_supported_symlink_rights(config: &io_test::HarnessConfig) -> fio::Rights {
    let mut rights = fio::Rights::GET_ATTRIBUTES | fio::Rights::READ_BYTES;
    if config.supports_mutable_file {
        rights |= fio::Rights::WRITE_BYTES;
    }
    if supports_mutable_attrs(&config) {
        rights |= fio::Rights::WRITE_BYTES;
    }
    if config.supports_executable_file {
        rights |= fio::Rights::EXECUTE;
    }
    rights
}

// Returns true if the harness supports at least one mutable attribute, false otherwise.
//
// *NOTE*: To allow testing both the io1 SetAttrs and io2 UpdateAttributes methods, harnesses
// that support mutable attributes must support [`fio::NodeAttributesQuery::CREATION_TIME`]
// and [`fio::NodeAttributesQuery::MODIFICATION_TIME`].
fn supports_mutable_attrs(config: &io_test::HarnessConfig) -> bool {
    if !config.supports_mutable_file && !config.supports_modify_directory {
        return false;
    }
    let all_mutable_attrs: fio::NodeAttributesQuery = fio::NodeAttributesQuery::ACCESS_TIME
        | fio::NodeAttributesQuery::MODIFICATION_TIME
        | fio::NodeAttributesQuery::CREATION_TIME
        | fio::NodeAttributesQuery::MODE
        | fio::NodeAttributesQuery::GID
        | fio::NodeAttributesQuery::UID
        | fio::NodeAttributesQuery::RDEV;
    if config.supported_attributes.intersects(all_mutable_attrs) {
        assert!(
            config.supported_attributes.contains(
                fio::NodeAttributesQuery::CREATION_TIME
                    | fio::NodeAttributesQuery::MODIFICATION_TIME
            ),
            "Harnesses must support at least CREATION_TIME if attributes are mutable."
        );
        return true;
    }
    false
}
