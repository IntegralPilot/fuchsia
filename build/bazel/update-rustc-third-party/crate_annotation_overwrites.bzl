# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Manual annotations for third-party Rust crates in Bazel.

These annotations are overrides for crates that require Bazel-specific
configuration (e.g. gen_build_script = False, custom Bazel target labels,
or platform feature selects) that cannot be derived solely from Cargo.toml.

For single-version crates, use version = "*" so that standard version bumps in
Cargo.toml do not cause annotations to be silently dropped.

NOTE: Merging Strategy - When the Python generator syncs this file, it
directly replaces generated Cargo.toml fields if a manual overwrite exists here.
For example, if both this file and Cargo.toml provide `rustc_flags`, the list
defined here __completely__ replaces the generated flags for that version.
Lists and dictionaries do NOT merge with annotations from Cargo.toml, so
include all necessary elements.
"""

load("@rules_rust//crate_universe:defs.bzl", "crate")

_TOKIO_HOST_FEATURES = [
    "bytes",
    "fs",
    "io-util",
    "libc",
    "mio",
    "net",
    "num_cpus",
    "process",
    "rt-multi-thread",
    "rt",
    "signal",
    "signal-hook-registry",
    "socket2",
    "sync",
    "time",
]

_TOKIO_HOST_DEPS = [
    "//third_party/rust_crates/ask2patch/memchr",
    "//third_party/rust_crates/forks/libc-0.2.189:libc",
    "//third_party/rust_crates/vendor/bytes-1.12.1:bytes",
    "//third_party/rust_crates/vendor/mio-1.2.1:mio",
    "//third_party/rust_crates/vendor/num_cpus-1.17.0:num_cpus",
    "//third_party/rust_crates/vendor/signal-hook-registry-1.4.8:signal_hook_registry",
    "//third_party/rust_crates/vendor/socket2-0.6.4:socket2",
]

CRATE_ANNOTATION_OVERWRITES = {
    "anyhow": [
        crate.annotation(
            version = "*",
            # TODO(https://github.com/rust-lang/rust/pull/99301): Re-enable this build script,
            # which adds `error_generic_member_access` that is currently unstable.
            gen_build_script = False,
        ),
    ],
    "nix": [
        crate.annotation(
            version = "*",
            gen_build_script = False,
        ),
    ],
    "tokio": [
        crate.annotation(
            version = "*",
            deps = crate.select(
                common = [],
                selects = {
                    "@rules_rust//rust/platform:x86_64-unknown-linux-gnu": _TOKIO_HOST_DEPS,
                    "@rules_rust//rust/platform:aarch64-unknown-linux-gnu": _TOKIO_HOST_DEPS,
                },
            ),
            crate_features = crate.select(
                common = [],
                # In Bazel, more specific select keys take precedence over more general ones
                # (e.g. @rules_rust//rust/platform:x86_64-unknown-linux-gnu over @platforms//os:linux).
                # The generated tokio target has feature select defined on the more specific
                # platform keys, so we must use those keys here to provide the features so they
                # don't get neglected.
                selects = {
                    "@rules_rust//rust/platform:x86_64-unknown-linux-gnu": _TOKIO_HOST_FEATURES,
                    "@rules_rust//rust/platform:aarch64-unknown-linux-gnu": _TOKIO_HOST_FEATURES,
                },
            ),
            rustc_flags = crate.select(
                common = [],
                selects = {
                    "@platforms//os:linux": ["--cfg=tokio_unstable"],
                },
            ),
        ),
    ],
    "proc-macro2": [
        crate.annotation(
            version = "*",
            # Build script will try to enable "--cfg=proc_macro_span", but proc_macro_span is still
            # an unstable feature.
            gen_build_script = False,
        ),
    ],
    "rand": [
        crate.annotation(
            version = "*",
            crate_features = [
                "alloc",
                "default",
                "std",
                "std_rng",
                "sys_rng",
                "thread_rng",
            ],
            deps = [
                "//third_party/rust_crates/vendor/chacha20-0.10.2:chacha20",
                "//third_party/rust_crates/vendor/getrandom-0.4.3:getrandom",
            ],
        ),
    ],
    "zerocopy": [
        crate.annotation(
            version = "*",
            crate_features = [
                "alloc",
                "std",
            ],
        ),
    ],
    "thiserror": [
        crate.annotation(
            version = "2.0.20",
            # TODO(https://github.com/rust-lang/rust/pull/99301): Making this exception for
            # an unstable feature is currently the only viable way we get thiserror to build in
            # the Fuchsia tree.
            rustc_flags = [
                "--cfg=error_generic_member_access",
                "-Zallow-features=error_generic_member_access",
            ],
        ),
        crate.annotation(
            version = "1.0.69",
            # TODO(https://github.com/rust-lang/rust/pull/99301): Re-enable this build script,
            # which adds `error_generic_member_access` that is currently unstable.
            gen_build_script = False,
        ),
    ],
    "ring": [
        crate.annotation(
            version = "*",
            # NOTE: Build script of this crate doesn't run due to missing
            # dependency. See https://fxbug.dev/345712835.
            gen_build_script = False,
            deps = [
                "//third_party/rust_crates/compat/ring-0.17.14:ring-core",
            ],
            rustc_env = {
                "RING_CORE_PREFIX": "ring_core_0_17_14_",
            },
        ),
    ],
    "rutabaga_gfx": [
        crate.annotation(
            version = "*",
            # Build script can add features we don't support.
            gen_build_script = False,
        ),
    ],
    "ahash": [
        crate.annotation(
            version = "*",
            # Build script can add features we don't support.
            gen_build_script = False,
        ),
    ],
    "mock-omaha-server": [
        crate.annotation(
            version = "*",
            deps = crate.select(
                common = [
                    "//src/lib/fuchsia-async",
                    "//src/lib/fuchsia-hyper",
                    "//src/lib/fuchsia-sync",
                    "//third_party/rust_crates/vendor:argh",
                ],
                selects = {
                    "@platforms//os:linux": [
                        "//third_party/rust_crates/vendor:tokio",
                    ],
                },
            ),
            rustc_flags = ["--cfg=fasync"],
        ),
    ],
    "pin-init": [
        crate.annotation(
            version = "*",
            gen_build_script = False,
            rustc_flags = [
                "--cfg=USE_RUSTC_FEATURES",
            ],
        ),
    ],
    "pin-init-internal": [
        crate.annotation(
            version = "*",
            gen_build_script = False,
            rustc_flags = [
                "--cfg=USE_RUSTC_FEATURES",
            ],
        ),
    ],
}
