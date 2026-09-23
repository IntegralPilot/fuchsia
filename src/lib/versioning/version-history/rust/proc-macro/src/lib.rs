// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use proc_macro::TokenStream;

#[proc_macro]
pub fn declare_version_history(_tokens: TokenStream) -> TokenStream {
    let versions = version_history::parse_version_history(version_history_data::VERSION_HISTORY)
        .expect("version-history.json to be parsed");
    if versions.is_empty() {
        panic!("version-history.json did not contain any versions");
    }

    let mut tokens = String::from("[");
    for version in versions {
        tokens.push_str(&format!(
            "Version {{ api_level: ApiLevel::from_u32({}), abi_revision: AbiRevision::from_u64({}), status: Status::{:?} }},",
            version.api_level.as_u32(), version.abi_revision.as_u64(), version.status
        ));
    }
    tokens.push_str("]");

    tokens.parse().unwrap()
}
