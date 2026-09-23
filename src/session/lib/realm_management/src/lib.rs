// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::ServerEnd;
use {
    fidl_fuchsia_component as fcomponent, fidl_fuchsia_component_decl as fdecl,
    fidl_fuchsia_io as fio,
};

/// Creates a (lazy) child in the specified `Realm`.
///
/// # Parameters
/// - `child_name`: The name of the child to be added.
/// - `child_url`: The component URL of the child to add.
/// - `collection_name`: The name of the collection to which the child will be added.
/// - `create_child_args`: Extra arguments passed to `Realm.CreateChild`.
/// - `realm`: The `Realm` to which the child will be added.
///
/// # Returns
/// `Ok` if the child is created successfully.
pub async fn create_child_component(
    child_name: &str,
    child_url: &str,
    collection_name: &str,
    create_child_args: fcomponent::CreateChildArgs,
    realm: &fcomponent::RealmProxy,
) -> Result<(), fcomponent::Error> {
    let collection_ref = fdecl::CollectionRef { name: collection_name.to_string() };
    let child_decl = fdecl::Child {
        name: Some(child_name.to_string()),
        url: Some(child_url.to_string()),
        startup: Some(fdecl::StartupMode::Lazy),
        environment: None,
        ..Default::default()
    };

    realm
        .create_child(&collection_ref, &child_decl, create_child_args)
        .await
        .map_err(|_| fcomponent::Error::Internal)?
}

/// Opens the exposed directory of a child in the specified `Realm`. This call
/// is expected to follow a matching call to `create_child`.
///
/// # Parameters
/// - `child_name`: The name of the child to bind.
/// - `collection_name`: The name of collection in which the child was created.
/// - `realm`: The `Realm` the child will bound in.
/// - `exposed_dir`: The server end on which to serve the exposed directory.
///
/// # Returns
/// `Ok` Result with a DirectoryProxy bound to the component's `exposed_dir`. This directory
/// contains the capabilities that the child exposed to its realm (as declared, for instance, in the
/// `expose` declaration of the component's `.cml` file).
pub async fn open_child_component_exposed_dir(
    child_name: &str,
    collection_name: &str,
    realm: &fcomponent::RealmProxy,
    exposed_dir: ServerEnd<fio::DirectoryMarker>,
) -> Result<(), fcomponent::Error> {
    let child_ref = fdecl::ChildRef {
        name: child_name.to_string(),
        collection: Some(collection_name.to_string()),
    };

    realm
        .open_exposed_dir(&child_ref, exposed_dir)
        .await
        .map_err(|_| fcomponent::Error::Internal)?
}

/// Destroys a child in the specified `Realm`. This call is expects a matching call to have been
/// made to `create_child`.
///
/// # Parameters
/// - `child_name`: The name of the child to destroy.
/// - `collection_name`: The name of collection in which the child was created.
/// - `realm`: The `Realm` the child will bound in.
///
/// # Errors
/// Returns an error if the child was not destroyed in the realm.
pub async fn destroy_child_component(
    child_name: &str,
    collection_name: &str,
    realm: &fcomponent::RealmProxy,
) -> Result<(), fcomponent::Error> {
    let child_ref = fdecl::ChildRef {
        name: child_name.to_string(),
        collection: Some(collection_name.to_string()),
    };

    realm.destroy_child(&child_ref).await.map_err(|_| fcomponent::Error::Internal)?
}

#[cfg(test)]
mod tests {
    use super::{
        create_child_component, destroy_child_component, open_child_component_exposed_dir,
    };
    use fidl::endpoints::{create_endpoints, create_proxy};
    use fidl_test_util::spawn_stream_handler;
    use session_testing::spawn_directory_server;
    use std::sync::LazyLock;
    use test_util::Counter;
    use {fidl_fuchsia_component as fcomponent, fidl_fuchsia_io as fio};

    /// Tests that creating a child results in the appropriate call to the `RealmProxy`.
    #[fuchsia::test]
    async fn create_child_parameters() {
        let child_name = "test_child";
        let child_url = "test_url";
        let child_collection = "test_collection";

        let realm_proxy = spawn_stream_handler(move |realm_request| async move {
            match realm_request {
                fcomponent::RealmRequest::CreateChild { collection, decl, args: _, responder } => {
                    assert_eq!(decl.name.unwrap(), child_name);
                    assert_eq!(decl.url.unwrap(), child_url);
                    assert_eq!(&collection.name, child_collection);

                    let _ = responder.send(Ok(()));
                }
                _ => panic!("Realm handler received an unexpected request"),
            }
        });

        assert!(
            create_child_component(
                child_name,
                child_url,
                child_collection,
                Default::default(),
                &realm_proxy
            )
            .await
            .is_ok()
        );
    }

    /// Tests that a success received when creating a child results in an appropriate result from
    /// `create_child`.
    #[fuchsia::test]
    async fn create_child_success() {
        let realm_proxy = spawn_stream_handler(move |realm_request| async move {
            match realm_request {
                fcomponent::RealmRequest::CreateChild {
                    collection: _,
                    decl: _,
                    args: _,
                    responder,
                } => {
                    let _ = responder.send(Ok(()));
                }
                _ => panic!("Realm handler received an unexpected request"),
            }
        });

        assert!(create_child_component("", "", "", Default::default(), &realm_proxy).await.is_ok());
    }

    /// Tests that an error received when creating a child results in an appropriate error from
    /// `create_child`.
    #[fuchsia::test]
    async fn create_child_error() {
        let realm_proxy = spawn_stream_handler(move |realm_request| async move {
            match realm_request {
                fcomponent::RealmRequest::CreateChild {
                    collection: _,
                    decl: _,
                    args: _,
                    responder,
                } => {
                    let _ = responder.send(Err(fcomponent::Error::Internal));
                }
                _ => panic!("Realm handler received an unexpected request"),
            }
        });

        assert!(
            create_child_component("", "", "", Default::default(), &realm_proxy).await.is_err()
        );
    }

    /// Tests that `open_child_component_exposed_dir` results in the appropriate call to `RealmProxy`.
    #[fuchsia::test]
    async fn open_child_component_exposed_dir_parameters() {
        let child_name = "test_child";
        let child_collection = "test_collection";

        let realm_proxy = spawn_stream_handler(move |realm_request| async move {
            match realm_request {
                fcomponent::RealmRequest::OpenExposedDir { child, exposed_dir: _, responder } => {
                    assert_eq!(child.name, child_name);
                    assert_eq!(child.collection, Some(child_collection.to_string()));

                    let _ = responder.send(Ok(()));
                }
                _ => panic!("Realm handler received an unexpected request"),
            }
        });

        let (_exposed_dir, exposed_dir_server_end) = create_endpoints::<fio::DirectoryMarker>();
        assert!(
            open_child_component_exposed_dir(
                child_name,
                child_collection,
                &realm_proxy,
                exposed_dir_server_end
            )
            .await
            .is_ok()
        );
    }

    /// Tests that a success received when opening a child's exposed directory
    /// results in an appropriate result from `open_child_component_exposed_dir`.
    #[fuchsia::test]
    async fn open_child_component_exposed_dir_success() {
        let realm_proxy = spawn_stream_handler(move |realm_request| async move {
            match realm_request {
                fcomponent::RealmRequest::OpenExposedDir {
                    child: _,
                    exposed_dir: _,
                    responder,
                } => {
                    let _ = responder.send(Ok(()));
                }
                _ => panic!("Realm handler received an unexpected request"),
            }
        });

        let (_exposed_dir, exposed_dir_server_end) = create_endpoints::<fio::DirectoryMarker>();
        assert!(
            open_child_component_exposed_dir("", "", &realm_proxy, exposed_dir_server_end)
                .await
                .is_ok()
        );
    }

    /// Tests that opening a child's exposed directory returns successfully.
    #[fuchsia::test]
    async fn open_child_exposed_dir_success() {
        // Make a static call counter to avoid unneeded complexity with cloned Arc<Mutex>.
        static CALL_COUNT: LazyLock<Counter> = LazyLock::new(|| Counter::new(0));

        let directory_request_handler = |directory_request| match directory_request {
            fio::DirectoryRequest::Open { path: fake_capability_path, .. } => {
                CALL_COUNT.inc();
                assert_eq!(fake_capability_path, "fake_capability_path");
            }
            _ => panic!("Directory handler received an unexpected request"),
        };

        let realm_proxy = spawn_stream_handler(move |realm_request| async move {
            match realm_request {
                fcomponent::RealmRequest::OpenExposedDir { child: _, exposed_dir, responder } => {
                    CALL_COUNT.inc();
                    spawn_directory_server(exposed_dir, directory_request_handler);
                    let _ = responder.send(Ok(()));
                }
                _ => panic!("Realm handler received an unexpected request"),
            }
        });

        let (exposed_dir, exposed_dir_server_end) = create_endpoints::<fio::DirectoryMarker>();
        open_child_component_exposed_dir("", "", &realm_proxy, exposed_dir_server_end)
            .await
            .unwrap();

        // Create a proxy of any FIDL protocol, with any `await`-able method.
        // (`fio::DirectoryMarker` here is arbitrary.)
        let (proxy, server_end) = create_proxy::<fio::DirectoryMarker>();

        // Connect should succeed, but it is still an asynchronous operation.
        // The `directory_request_handler` is not called yet.
        assert!(
            fdio::service_connect_at(
                &exposed_dir.into_channel(),
                "fake_capability_path",
                server_end.into_channel()
            )
            .is_ok()
        );

        // Attempting to invoke and await an arbitrary method to ensure the
        // `directory_request_handler` responds to the Open() method and increment
        // the DIRECTORY_OPEN_CALL_COUNT.
        //
        // Since this is a fake capability (of any arbitrary type), it should fail.
        assert!(proxy.rewind().await.is_err());

        // Calls to Realm::OpenExposedDir and Directory::Open should have happened.
        assert_eq!(CALL_COUNT.get(), 2);
    }

    /// Tests that an error received when opening a child's exposed directory
    /// results in an appropriate error from `open_exposed_dir`.
    #[fuchsia::test]
    async fn open_child_component_exposed_dir_error() {
        let realm_proxy = spawn_stream_handler(move |realm_request| async move {
            match realm_request {
                fcomponent::RealmRequest::OpenExposedDir {
                    child: _,
                    exposed_dir: _,
                    responder,
                } => {
                    let _ = responder.send(Err(fcomponent::Error::Internal));
                }
                _ => panic!("Realm handler received an unexpected request"),
            }
        });

        let (_exposed_dir, exposed_dir_server_end) = create_endpoints::<fio::DirectoryMarker>();
        assert!(
            open_child_component_exposed_dir("", "", &realm_proxy, exposed_dir_server_end)
                .await
                .is_err()
        );
    }

    /// Tests that `destroy_child` results in the appropriate call to `RealmProxy`.
    #[fuchsia::test]
    async fn destroy_child_parameters() {
        let child_name = "test_child";
        let child_collection = "test_collection";

        let realm_proxy = spawn_stream_handler(move |realm_request| async move {
            match realm_request {
                fcomponent::RealmRequest::DestroyChild { child, responder } => {
                    assert_eq!(child.name, child_name);
                    assert_eq!(child.collection, Some(child_collection.to_string()));

                    let _ = responder.send(Ok(()));
                }
                _ => panic!("Realm handler received an unexpected request"),
            }
        });

        assert!(destroy_child_component(child_name, child_collection, &realm_proxy).await.is_ok());
    }
}
