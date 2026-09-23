// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdio::Namespace;
use fidl::endpoints::Proxy;
use fidl_fuchsia_component as fcomponent;
use fuchsia_component::client::connect_to_protocol;
use std::fmt::Debug;
use uuid::Uuid;

#[cfg(fuchsia_api_level_at_least = "HEAD")]
use fidl_fuchsia_component_sandbox as _;
#[cfg(fuchsia_api_level_less_than = "HEAD")]
use {fidl::endpoints::ClientEnd, fidl_fuchsia_component_sandbox as fsandbox};

mod error;
pub use error::Error;

/// A thin wrapper that represents the namespace created by [extend_namespace].
///
/// Users can obtain the path to the namespace from [InstalledNamespace::prefix] and pass that
/// to capability connection APIs such as [fuchsia-component]'s [client::connect_to_protocol_at]
/// to access capabilities in the namespace.
///
/// Furthermore, the [InstalledNamespace] acts as an RAII container for the capabilities. When
/// the [InstalledNamespace] is dropped, the test realm factory server may free state associated
/// with serving those capabilities. Therefore, the test should only drop this once it no longer
/// needs to connect to the capabilities or needs activity performed on their behalf.
pub struct InstalledNamespace {
    prefix: String,

    /// This is not used, but it keeps the RealmFactory connection alive.
    ///
    /// The RealmFactory server may use this connection to pin the lifetime of the realm created
    /// for the test.
    _realm_factory: fidl::AsyncChannel,
}

impl InstalledNamespace {
    pub fn prefix(&self) -> &str {
        &self.prefix
    }
}

impl Drop for InstalledNamespace {
    fn drop(&mut self) {
        let Ok(namespace) = Namespace::installed() else {
            return;
        };
        let _ = namespace.unbind(&self.prefix);
    }
}

impl AsRef<str> for &InstalledNamespace {
    fn as_ref(&self) -> &str {
        self.prefix()
    }
}

/// Converts the given dictionary to a namespace and adds it this component's namespace,
/// thinly wrapped by the returned [InstalledNamespace].
///
/// Users can obtain the path to the namespace from [InstalledNamespace::prefix] and pass that
/// to capability connection APIs such as [fuchsia-component]'s [client::connect_to_protocol_at]
/// to access capabilities in the namespace.
///
/// Furthermore, the [InstalledNamespace] acts as an RAII container for the capabilities. When
/// the [InstalledNamespace] is dropped, the test realm factory server may free state associated
/// with serving those capabilities. Therefore, the test should only drop this once it no longer
/// needs to connect to the capabilities or needs activity performed on their behalf.
pub async fn extend_namespace<T>(
    realm_factory: T,
    #[cfg(fuchsia_api_level_at_least = "HEAD")] capability: zx::EventPair,
    #[cfg(fuchsia_api_level_less_than = "HEAD")] dictionary: ClientEnd<fsandbox::DictionaryMarker>,
) -> Result<InstalledNamespace, error::Error>
where
    T: Proxy + Debug,
{
    let namespace_proxy = connect_to_protocol::<fcomponent::NamespaceMarker>()
        .map_err(|e| error::Error::ConnectionFailed(format!("{:?}", e)))?;
    // TODO(https://fxbug.dev/336392298): What should we use for
    // the namespace's unique id? Could also consider an atomic counter,
    // or the name of the test.
    let prefix = format!("/dict-{}", Uuid::new_v4());
    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    let mut namespace_entries = {
        let dicts =
            vec![fcomponent::NamespaceInputEntry2 { path: prefix.clone().into(), capability }];
        namespace_proxy.create2(dicts).await?.map_err(error::Error::NamespaceCreation)?
    };
    #[cfg(fuchsia_api_level_less_than = "HEAD")]
    let mut namespace_entries = {
        let dicts =
            vec![fcomponent::NamespaceInputEntry { path: prefix.clone().into(), dictionary }];
        namespace_proxy.create(dicts).await?.map_err(error::Error::NamespaceCreation)?
    };
    let namespace = Namespace::installed().map_err(error::Error::NamespaceNotInstalled)?;
    let count = namespace_entries.len();
    if count != 1 {
        return Err(error::Error::InvalidNamespaceEntryCount { prefix, count });
    }
    let entry = namespace_entries.remove(0);
    if entry.path.is_none() || entry.directory.is_none() {
        return Err(error::Error::EntryIncomplete { prefix, message: format!("{:?}", entry) });
    }
    if entry.path.as_ref().unwrap() != &prefix {
        return Err(error::Error::PrefixDoesNotMatchPath {
            prefix,
            path: format!("{:?}", entry.path),
        });
    }
    namespace.bind(&prefix, entry.directory.unwrap()).map_err(error::Error::NamespaceBind)?;
    Ok(InstalledNamespace { prefix, _realm_factory: realm_factory.into_channel().unwrap() })
}
