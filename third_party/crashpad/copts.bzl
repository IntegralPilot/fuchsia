# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Common compiler options."""

load(":includes.bzl", "CRASHPAD_COMPAT_INCLUDES_COPTS")

# Keep in sync with config("unicode") from Chromium's build/config/win/BUILD.gn:
# https://source.chromium.org/chromium/chromium/src/+/main:build/config/win/BUILD.gn;l=570;drc=804d5a91d49d0ad79d3d5529e6ba2610225cfe55
CRASHPAD_WINDOWS_UNICODE_COPTS = [
    "-DUNICODE",
    "-D_UNICODE",
]

# Keep in sync with config("winver") from Chromium's build/config/win/BUILD.gn:
# https://source.chromium.org/chromium/chromium/src/+/main:build/config/win/BUILD.gn;l=287;drc=804d5a91d49d0ad79d3d5529e6ba2610225cfe55
CRASHPAD_WINDOWS_WINVER_COPTS = [
    "-DNTDDI_VERSION=NTDDI_WIN10_FE",
    "-D_WIN32_WINNT=0x0A00",
    "-DWINVER=0x0A00",
]

# Keep in sync with config("nominmax") from Chromium's build/config/win/BUILD.gn:
# https://source.chromium.org/chromium/chromium/src/+/main:build/config/win/BUILD.gn;l=592;drc=804d5a91d49d0ad79d3d5529e6ba2610225cfe55
CRASHPAD_WINDOWS_NOMINMAX_COPTS = [
    "-DNOMINMAX",
]

# Keep in sync with config("lean_and_mean") from Chromium's build/config/win/BUILD.gn:
# https://source.chromium.org/chromium/chromium/src/+/main:build/config/win/BUILD.gn;l=582;drc=804d5a91d49d0ad79d3d5529e6ba2610225cfe55
CRASHPAD_WINDOWS_LEAN_AND_MEAN_COPTS = [
    "-DWIN32_LEAN_AND_MEAN",
]

# Keep in sync with config("runtime_library") from Chromium's build/config/win/BUILD.gn:
# https://source.chromium.org/chromium/chromium/src/+/main:build/config/win/BUILD.gn;l=216;drc=804d5a91d49d0ad79d3d5529e6ba2610225cfe55
CRASHPAD_WINDOWS_RUNTIME_LIBRARY_COPTS = [
    "-D__STD_C",
    "-D_CRT_RAND_S",
    "-D_CRT_SECURE_NO_DEPRECATE",
    "-D_SCL_SECURE_NO_DEPRECATE",
    "-D_ATL_NO_OPENGL",
    "-D_WINDOWS",
    "-DCERT_CHAIN_PARA_HAS_EXTRA_FIELDS",
    "-DPSAPI_VERSION=2",
    "-DWIN32",
    "-D_SECURE_ATL",
    "-DWINAPI_FAMILY=WINAPI_FAMILY_DESKTOP_APP",
]

# Keep in sync with config("no_exceptions") from Chromium's build/config/compiler/BUILD.gn:
# https://source.chromium.org/chromium/chromium/src/+/main:build/config/compiler/BUILD.gn;l=1862;drc=cf59bae1055c82caefd5c5f95250ee5f3b4ee05b
CRASHPAD_WINDOWS_NO_EXCEPTIONS_COPTS = [
    "-D_HAS_EXCEPTIONS=0",
]

CRASHPAD_MINI_CHROMIUM_INCLUDES = [
    "-Ithird_party/mini_chromium/mini_chromium",
]

WINDOWS_COPTS = CRASHPAD_MINI_CHROMIUM_INCLUDES + CRASHPAD_WINDOWS_UNICODE_COPTS + CRASHPAD_WINDOWS_WINVER_COPTS + CRASHPAD_WINDOWS_NOMINMAX_COPTS + CRASHPAD_WINDOWS_LEAN_AND_MEAN_COPTS + CRASHPAD_WINDOWS_RUNTIME_LIBRARY_COPTS + CRASHPAD_WINDOWS_NO_EXCEPTIONS_COPTS

DEFAULT_COPTS = CRASHPAD_MINI_CHROMIUM_INCLUDES + ["-Wa,--noexecstack", "-Wno-non-virtual-dtor"]

CRASHPAD_COMMON_COPTS_SELECTOR = {
    "//conditions:default": DEFAULT_COPTS,
}

CRASHPAD_COMMON_COPTS = select(CRASHPAD_COMMON_COPTS_SELECTOR) + CRASHPAD_COMPAT_INCLUDES_COPTS
