// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// This struct configures the example subsystem which is used for
/// documentation and testing. The example subsystem is only configured
/// when the example enabled option is set.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
pub struct ExampleConfig {
    // Whether to include the example AIB in the build.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub include_example_aib: bool,
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_example_config_deserialize() {
        let json = serde_json::json!({
            "include_example_aib": true,
        });

        let parsed: ExampleConfig = serde_json::from_value(json).unwrap();
        let expected = ExampleConfig { include_example_aib: true };

        assert_eq!(parsed, expected);
    }
}
