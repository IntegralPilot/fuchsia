# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

visibility(["//build/bazel/rules/idk/private"])

"""Lists of targets included in the Fuchsia IDK."""

STABLE_CC_SOURCE_LIBRARY_ATOMS = [
    # buildifier: keep sorted
    "//sdk/lib/async:async-cpp_idk",
    "//sdk/lib/async:async_idk",
    "//sdk/lib/async-loop:async-loop-cpp_idk",
    "//sdk/lib/async-loop:async-loop_idk",
    "//sdk/lib/component/incoming/cpp:cpp_idk",
    "//sdk/lib/component/outgoing/cpp:cpp_idk",
    "//sdk/lib/fidl:fidl_idk",
    "//sdk/lib/fidl/cpp:cpp_base_idk",
    "//sdk/lib/fidl/cpp:cpp_idk",
    "//sdk/lib/fidl/cpp:hlcpp_conversion_idk",
    "//sdk/lib/fidl/cpp:natural_ostream_idk",
    "//sdk/lib/fidl/cpp/wire:wire_idk",
    "//sdk/lib/fidl/hlcpp:hlcpp_base_idk",
    "//sdk/lib/fidl/hlcpp:hlcpp_idk",
    "//sdk/lib/fidl/hlcpp:hlcpp_sync_idk",
    "//sdk/lib/fidl_base:fidl_base_idk",
    "//sdk/lib/fit:fit_idk",
    "//sdk/lib/fit-promise:fit-promise_idk",
    "//sdk/lib/magma_common:magma_common_idk",
    "//sdk/lib/media/cpp:no_converters_idk",
    "//sdk/lib/stdcompat:stdcompat_idk",
    "//sdk/lib/syslog/cpp:cpp_idk",
    "//sdk/lib/syslog/structured_backend:structured_backend_idk",
    "//sdk/lib/utf-utils:utf-utils_idk",
    "//zircon/system/ulib/sync:sync-cpp_idk",
    "//zircon/system/ulib/zx:zx_idk",
]

UNSTABLE_CC_SOURCE_LIBRARY_ATOMS = [
    # buildifier: keep sorted
    "//sdk/lib/memory_barriers:memory_barriers_idk",
]

ALL_CC_SOURCE_LIBRARY_ATOMS = STABLE_CC_SOURCE_LIBRARY_ATOMS + UNSTABLE_CC_SOURCE_LIBRARY_ATOMS

CC_PREBUILT_SHARED_LIBRARY_ATOMS = [
    # buildifier: keep sorted
    "//sdk/lib/async-default:async-default_idk",
    "//sdk/lib/fdio:fdio_idk",
    "//sdk/lib/svc:svc_idk",
    "//sdk/lib/syslog/cpp:backend_fuchsia_globals_idk",
]

CC_PREBUILT_STATIC_LIBRARY_ATOMS = [
    # buildifier: keep sorted
    "//sdk/lib/async-loop:async-loop-default_idk",
    "//zircon/system/ulib/sync:sync_idk",
]

ALL_CC_PREBUILT_LIBRARY_ATOMS = CC_PREBUILT_SHARED_LIBRARY_ATOMS + CC_PREBUILT_STATIC_LIBRARY_ATOMS

DATA_ATOMS = [
    # buildifier: keep sorted
]

# Lists of FIDL targets are defined in //sdk/fidl/BUILD.bazel.

BUILD_HOST_TOOLS_ATOMS = [
    # buildifier: keep sorted
    "//tools/fidl/fidlc:fidl-format_idk",
    "//tools/fidl/fidlc:fidlc_idk",
    "//tools/fidl/fidlgen_cpp:fidlgen_cpp_idk",
    "//tools/fidl/fidlgen_hlcpp:fidlgen_hlcpp_idk",
    "//tools/orchestrate:orchestrate_idk",
]

_NON_BUILD_HOST_TOOLS_ATOMS_REQUIRED_BY_BAZEL_SDK = [
    # buildifier: keep sorted
    "//zircon/tools/merkleroot:merkleroot_idk",
]

NON_BUILD_HOST_TOOLS_ATOMS = _NON_BUILD_HOST_TOOLS_ATOMS_REQUIRED_BY_BAZEL_SDK + [
    # buildifier: keep sorted
    "//src/sys/pkg/testing/fake-omaha-client:fake-omaha-client_idk",
    "//tools/net/device-finder:device-finder_idk",
]

ALL_HOST_TOOL_ATOMS = BUILD_HOST_TOOLS_ATOMS + NON_BUILD_HOST_TOOLS_ATOMS

NOOP_ATOMS_LIST = [
    # buildifier: keep sorted
    "//sdk/lib/zircon-assert:zircon-assert_idk",
    "//src/zircon/lib/zircon:zircon_idk",
]
