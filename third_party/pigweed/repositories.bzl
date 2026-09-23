# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

def setup_pigweed_repository_dependencies():
    # Note that this is a fake repository that only contains the minimum
    # amount of declarations required by Bazel to run. The Pigweed repository
    # contains optional references to @freertos that cause `bazel query` to fail.
    native.local_repository(
        name = "freertos",
        path = "third_party/pigweed/bazel/freertos",
    )

    # Note that this is a fake repository that only contains the minimum
    # amount of declarations required by Bazel to run. The Pigweed repository
    # contains optional references to @hal_driver that cause `bazel query` to fail.
    native.local_repository(
        name = "hal_driver",
        path = "third_party/pigweed/bazel/hal_driver",
    )


    # Note that this is a fake repository that only contains the minimum
    # amount of declarations required by Bazel to run. The Pigweed repository
    # contains optional references to @pico-sdk that cause `bazel query` to fail.
    native.local_repository(
        name = "pico-sdk",
        path = "third_party/pigweed/bazel/pico-sdk",
    )
