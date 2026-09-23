# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""
Providers for Fuchsia product assembly.
"""

FuchsiaAssembledPackageInfo = provider(
    "Packages that can be included into a product. It consists of the package and the corresponding config data.",
    fields = {
        "package": "The base package",
        "configs": "A list of configs that is attached to packages",
        "files": "Files needed by package and config files.",
        "build_id_dirs": "Directories containing the debug symbols",
    },
)

FuchsiaProductInputBundleInfo = provider(
    doc = "A product input bundle info used to contain the product input bundle directory",
    fields = {
        "directory": "Directory of the product input bundle container",
        "build_id_dirs": "Directories containing the debug symbols",
    },
)

FuchsiaProductConfigInfo = provider(
    doc = "Info about the ProductConfiguration and its directory containing the product_config.json and all deps.",
    fields = {
        "directory": "Directory of the product config container",
        "build_type": "The build type of the product.",
        "build_id_dirs": "Directories containing the debug symbols",
        "enable_example_aib": "Whether example AIBs are enabled for this product.",
    },
)

FuchsiaOmahaOtaConfigInfo = provider(
    doc = "OTA configuration data for products that use the Omaha client.",
    fields = {
        "channels": "The omaha channel configuration data.",
        "tuf_repositories": "A dict of TUF repository configurations, by hostname.",
    },
)

FuchsiaStarnixContainerInfo = provider(
    doc = "Fields needed to generate a starnix container",
    fields = {
        "name": "Name of the starnix container",
        "base": "Name of package containing base resources to include",
        "hals": "List of HAL package names",
        "skip_subpackages": "Whether to skip inlcuding HALs as subpackages",
        "system": "Path to system image",
        "vendor": "Path to vendor image",
        "ramdisk": "Path to ramdisk image",
        "fstab": "Path to fstab will go in /odm which overrides the one in /vendor",
        "init": "Path to extra init scripts, will go in /odm/etc/init. Can be passed more than once.",
        "system_file_overwrite_srcs": "List of paths to files to overwrite",
        "system_file_overwrite_dsts": "List of destination paths for file overwrites",
        "system_file_create_srcs": "List of paths to files to create",
        "system_file_create_dsts": "List of destination paths for file creates",
        "system_file_override_deletions": "List of paths to files to delete",
    },
)

PlatformArtifactsInfo = provider(
    doc = "A set of platform artifacts used by product assembly.",
    fields = {
        "root": "The root directory for these artifacts",
        "files": "All files contained in the bundle",
    },
)

AssemblyInputBundleInfo = provider(
    doc = "A set of assembly inputs used by product assembly.",
    fields = {
        "name": "The name of the Assembly Input Bundle",
        "directory": "The directory to the Assembly Input Bundle",
    },
)
