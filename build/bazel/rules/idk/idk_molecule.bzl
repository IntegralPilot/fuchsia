# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Forwarding definitions for IDK molecules."""

load(
    "//build/bazel/rules/idk/private:idk_molecule.bzl",
    _idk_all_api_level_and_cpu_combinations_molecule = "idk_all_api_level_and_cpu_combinations_molecule",
    _idk_host_tool_molecule_for_configured_host_cpus = "idk_host_tool_molecule_for_configured_host_cpus",
    _idk_host_tool_molecule_for_current_host_cpu = "idk_host_tool_molecule_for_current_host_cpu",
    _idk_molecule = "idk_molecule",
)

idk_molecule = _idk_molecule
idk_all_api_level_and_cpu_combinations_molecule = _idk_all_api_level_and_cpu_combinations_molecule
idk_host_tool_molecule_for_current_host_cpu = _idk_host_tool_molecule_for_current_host_cpu
idk_host_tool_molecule_for_configured_host_cpus = _idk_host_tool_molecule_for_configured_host_cpus
