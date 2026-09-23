# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

load(":providers.bzl", "FuchsiaPackageInfo")

def get_component_manifests(package):
    """Returns a list of the component manifest paths for all components in the package.

    Args:
        package: The package to parse.
    """
    return [entry.dest for entry in package[FuchsiaPackageInfo].packaged_components]

def get_driver_component_manifests(package):
    """Returns a list of the component manifest paths for all driver components in the package.

    Args:
        package: The package to parse.
    """
    return [entry.dest for entry in package[FuchsiaPackageInfo].packaged_components if entry.component_info.is_driver]
