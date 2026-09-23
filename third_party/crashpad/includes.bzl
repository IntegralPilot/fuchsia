# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Inclusion entries needed for compat headers."""

CRASHPAD_COMPAT_INCLUDES_COPTS_MACOS = [
    "-Ithird_party/crashpad/src/compat/mac",
]

CRASHPAD_COMPAT_INCLUDES_COPTS_IOS = [
    "-Ithird_party/crashpad/src/compat/mac",
    "-Ithird_party/crashpad/src/compat/ios",
]

CRASHPAD_COMPAT_INCLUDES_COPTS_ANDROID = [
    "-Ithird_party/crashpad/src/compat/linux",
    "-Ithird_party/crashpad/src/compat/android",
]

CRASHPAD_COMPAT_INCLUDES_COPTS_DEFAULT = [
    "-Ithird_party/crashpad/src/compat/linux",
]

CRASHPAD_COMPAT_INCLUDES_COPTS = select({
    "@platforms//os:osx": CRASHPAD_COMPAT_INCLUDES_COPTS_MACOS,
    "//conditions:default": CRASHPAD_COMPAT_INCLUDES_COPTS_DEFAULT,
})
