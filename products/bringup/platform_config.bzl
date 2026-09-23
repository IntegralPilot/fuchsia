# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

load(
    "@rules_fuchsia//fuchsia:assembly.bzl",
    "BUILD_TYPES",
)

PLATFORM_CONFIG_BASE_JSON = {
    "feature_set_level": "bootstrap",
    "build_type": BUILD_TYPES.ENG,
    "kernel": {
        "oom": {
            "behavior": "job_kill",
        },
        "scheduler_enable_new_wakeup_accounting": True,
    },
    "development_support": {
        "include_netsvc": True,
    },
    "storage": {
        "filesystems": {
            "image_mode": "no_image",
        },
        # Bootstrap configuration does not contain the real
        # system-update-committer, so we request the auto-slot-committer instead
        # to prevent slots being marked as unbootable.
        "auto_slot_committer": True,
    },
}

PLATFORM_CONFIG_WITH_TEST_JSON = PLATFORM_CONFIG_BASE_JSON | {
    "development_support": {
        "include_bootstrap_testing_framework": True,
    },
    "power": {
        "enable_non_hermetic_testing": True,
    },
}
