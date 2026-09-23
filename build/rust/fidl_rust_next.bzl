# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# LINT.IfChange
# The new Rust bindings are currently in a phased rollout. This allowlist
# controls where in the tree generated bindings may be used from.
fidl_rust_next_allowlist = [
    "//examples:__subpackages__",
    "//examples/fidl/new/key_value_store/use_generic_values/rust_next:__subpackages__",
    "//examples/fidl/rust_next:__subpackages__",
    "//sdk/fidl:__subpackages__",
    "//sdk/lib/async:__subpackages__",
    "//sdk/lib/driver:__subpackages__",
    "//src/bringup/lib/userboot:__subpackages__",
    "//src/connectivity/bluetooth:__subpackages__",
    "//src/connectivity/overnet:__subpackages__",
    "//src/connectivity/wlan:__subpackages__",
    "//src/devices:__subpackages__",
    "//src/graphics:__subpackages__",
    "//src/lib/fdomain:__subpackages__",
    "//src/lib/fidl/rust_next:__subpackages__",
    "//src/lib/fuchsia-component:__subpackages__",
    "//src/tests/fidl/conformance_suite:__subpackages__",
    "//src/ui/input:__subpackages__",
    "//src/ui/lib/input_pipeline:__subpackages__",
    "//src/ui/tools/print-input-report-new:__subpackages__",
    "//tools/fidl:__subpackages__",
    "//vendor/google:__subpackages__",
    "//zircon/kernel/lib/userabi/userboot:__subpackages__",
]
# LINT.ThenChange(fidl_rust_next.gni)
