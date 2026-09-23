// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(unused_imports)]

use android_system_microfuchsia_vm_service::{aidl::android::system::microfuchsia::vm_service::{IHostProxy::IHostProxy, IMicrofuchsia::{self, BnMicrofuchsia}}, binder::{BinderFeatures, SpIBinder}};
use binder;
use std::fs;
use fuchsia_sync::Mutex;

pub struct Microfuchsia {
    host_proxy: Mutex<Option<binder::Strong<dyn IHostProxy>>>,
}

impl binder::Interface for Microfuchsia {}

impl IMicrofuchsia::IMicrofuchsia for Microfuchsia {
    fn setHostProxy(&self, host_proxy: &binder::Strong<dyn IHostProxy>) -> binder::Result<()> {
        *self.host_proxy.lock() = Some(host_proxy.clone());
        binder::Result::Ok(())
    }

    fn trustedAppUuids(&self) -> binder::Result<Vec<String>> {
        // TODO: Sort out which errors should be returned to the caller and translate them
        // to the proper binder errors. If we're completely misconfigured then we probably do not
        // want to continue.
        let uuids = fs::read_dir("/ta")
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .map(|s| s.into_string().unwrap())
            .collect::<Vec<_>>();
        binder::Result::Ok(uuids)
    }
}

impl Microfuchsia {
    pub fn new() -> Self {
        Self { host_proxy: Mutex::new(None) }
    }
}

pub fn new_binder() -> SpIBinder {
    let accessor = Microfuchsia::new();
    BnMicrofuchsia::new_binder(accessor, BinderFeatures::default()).as_binder()
}
