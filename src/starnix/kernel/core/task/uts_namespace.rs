// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::vfs::FsString;
use starnix_sync::{LockDepRwLock, UtsNamespaceLock};
use std::sync::Arc;

const DEFAULT_HOST_NAME: &str = "localhost";
const DEFAULT_DOMAIN_NAME: &str = "localdomain";

pub type UtsNamespaceHandle = Arc<LockDepRwLock<UtsNamespace, UtsNamespaceLock>>;

// Unix Time-sharing Namespace (UTS) information.
// Stores the hostname and domainname for a specific process.
//
// See https://man7.org/linux/man-pages/man7/uts_namespaces.7.html
#[derive(Clone)]
pub struct UtsNamespace {
    pub hostname: FsString,
    pub domainname: FsString,
}

impl UtsNamespace {
    pub fn fork(&self) -> UtsNamespaceHandle {
        Arc::new(self.clone().into())
    }
}

impl Default for UtsNamespace {
    fn default() -> Self {
        Self { hostname: DEFAULT_HOST_NAME.into(), domainname: DEFAULT_DOMAIN_NAME.into() }
    }
}
