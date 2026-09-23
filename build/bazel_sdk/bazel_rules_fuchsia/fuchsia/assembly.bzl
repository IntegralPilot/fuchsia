# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Public definitions for Assembly related rules."""

load(
    "@fuchsia_rules_common//assembly:product_configuration.bzl",
    _BUILD_TYPES = "BUILD_TYPES",
)
load(
    "//fuchsia/private:fuchsia_prebuilt_package.bzl",
    _fuchsia_prebuilt_package = "fuchsia_prebuilt_package",
)
load(
    "//fuchsia/private/assembly:fuchsia_assembly_developer_overrides.bzl",
    _fuchsia_assembly_developer_overrides_list = "fuchsia_assembly_developer_overrides_list",
    _fuchsia_prebuilt_assembly_developer_overrides = "fuchsia_prebuilt_assembly_developer_overrides",
)
load(
    "//fuchsia/private/assembly:fuchsia_board_configuration.bzl",
    _fuchsia_board_configuration = "fuchsia_board_configuration",
    _fuchsia_hybrid_board_configuration = "fuchsia_hybrid_board_configuration",
    _fuchsia_prebuilt_board_configuration = "fuchsia_prebuilt_board_configuration",
)
load(
    "//fuchsia/private/assembly:fuchsia_board_input_bundle.bzl",
    _INCLUDE_IN = "INCLUDE_IN",
    _fuchsia_board_input_bundle = "fuchsia_board_input_bundle",
    _fuchsia_prebuilt_board_input_bundle = "fuchsia_prebuilt_board_input_bundle",
)
load(
    "//fuchsia/private/assembly:fuchsia_board_input_bundle_set.bzl",
    _fuchsia_board_input_bundle_set = "fuchsia_board_input_bundle_set",
    _fuchsia_prebuilt_board_input_bundle_set = "fuchsia_prebuilt_board_input_bundle_set",
)
load(
    "//fuchsia/private/assembly:fuchsia_bootloader_partition.bzl",
    _fuchsia_bootloader_partition = "fuchsia_bootloader_partition",
)
load(
    "//fuchsia/private/assembly:fuchsia_bootstrap_partition.bzl",
    _fuchsia_bootstrap_partition = "fuchsia_bootstrap_partition",
)
load(
    "//fuchsia/private/assembly:fuchsia_elf_sizes.bzl",
    _fuchsia_elf_sizes = "fuchsia_elf_sizes",
)
load(
    "//fuchsia/private/assembly:fuchsia_gen_android_starnix_container.bzl",
    _fuchsia_gen_android_starnix_container = "fuchsia_gen_android_starnix_container",
)
load(
    "//fuchsia/private/assembly:fuchsia_package_directory.bzl",
    _fuchsia_package_directory = "fuchsia_package_directory",
)
load(
    "//fuchsia/private/assembly:fuchsia_package_size_check.bzl",
    _fuchsia_package_size_check = "fuchsia_package_size_check",
)
load(
    "//fuchsia/private/assembly:fuchsia_package_with_configs.bzl",
    _fuchsia_package_with_configs = "fuchsia_package_with_configs",
)
load(
    "//fuchsia/private/assembly:fuchsia_partition.bzl",
    _PARTITION_TYPE = "PARTITION_TYPE",
    _SLOT = "SLOT",
    _fuchsia_partition = "fuchsia_partition",
)
load(
    "//fuchsia/private/assembly:fuchsia_partitions_configuration.bzl",
    _fuchsia_partitions_configuration = "fuchsia_partitions_configuration",
    _fuchsia_prebuilt_partitions_configuration = "fuchsia_prebuilt_partitions_configuration",
    _fuchsia_ssh_key_upload_method_inline = "fuchsia_ssh_key_upload_method_inline",
    _fuchsia_ssh_key_upload_method_staged = "fuchsia_ssh_key_upload_method_staged",
)
load(
    "//fuchsia/private/assembly:fuchsia_platform_artifacts.bzl",
    _fuchsia_platform_artifacts = "fuchsia_platform_artifacts",
)
load(
    "//fuchsia/private/assembly:fuchsia_post_processing_script.bzl",
    _fuchsia_post_processing_script = "fuchsia_post_processing_script",
)
load(
    "//fuchsia/private/assembly:fuchsia_product.bzl",
    _fuchsia_product = "fuchsia_product",
)
load(
    "//fuchsia/private/assembly:fuchsia_product_bundle.bzl",
    _DELIVERY_BLOB_TYPE = "DELIVERY_BLOB_TYPE",
    _fuchsia_product_bundle = "fuchsia_product_bundle",
)
load(
    "//fuchsia/private/assembly:fuchsia_product_configuration.bzl",
    _INPUT_DEVICE_TYPE = "INPUT_DEVICE_TYPE",
    _fuchsia_hybrid_product_configuration = "fuchsia_hybrid_product_configuration",
    _fuchsia_prebuilt_product_configuration = "fuchsia_prebuilt_product_configuration",
    _fuchsia_prebuilt_product_configuration_extract_package = "fuchsia_prebuilt_product_configuration_extract_package",
    _fuchsia_product_configuration = "fuchsia_product_configuration",
)
load(
    "//fuchsia/private/assembly:fuchsia_product_input_bundle.bzl",
    _fuchsia_prebuilt_product_input_bundle = "fuchsia_prebuilt_product_input_bundle",
    _fuchsia_product_input_bundle = "fuchsia_product_input_bundle",
)
load(
    "//fuchsia/private/assembly:fuchsia_product_ota_config.bzl",
    _fuchsia_product_ota_config = "fuchsia_product_ota_config",
    _ota_realm = "ota_realm",
    _tuf_repo = "tuf_repo",
    _tuf_repo_root = "tuf_repo_root",
)
load(
    "//fuchsia/private/assembly:fuchsia_product_size_check.bzl",
    _fuchsia_product_size_check = "fuchsia_product_size_check",
)
load(
    "//fuchsia/private/assembly:fuchsia_repository_keys.bzl",
    _fuchsia_repository_keys = "fuchsia_repository_keys",
)
load(
    "//fuchsia/private/assembly:fuchsia_scrutiny_config.bzl",
    _SCRUTINY_VERIFIERS = "SCRUTINY_VERIFIERS",
    _fuchsia_scrutiny_config = "fuchsia_scrutiny_config",
)
load(
    "//fuchsia/private/assembly:fuchsia_size_report_aggregator.bzl",
    _fuchsia_size_report_aggregator = "fuchsia_size_report_aggregator",
)
load(
    "//fuchsia/private/assembly:fuchsia_starnix_container.bzl",
    _fuchsia_starnix_container = "fuchsia_starnix_container",
    _fuchsia_starnix_file_create = "fuchsia_starnix_file_create",
    _fuchsia_starnix_file_delete = "fuchsia_starnix_file_delete",
    _fuchsia_starnix_file_override = "fuchsia_starnix_file_override",
)
load(
    "//fuchsia/private/assembly:fuchsia_test_product_bundle.bzl",
    _fuchsia_test_product_bundle = "fuchsia_test_product_bundle",
)
load(
    "//fuchsia/private/assembly:fuchsia_update_package.bzl",
    _fuchsia_update_package = "fuchsia_update_package",
)
load(
    "//fuchsia/private/assembly:fuchsia_virtual_device.bzl",
    _ARCH = "ARCH",
    _DEFAULT_PORTS = "DEFAULT_PORTS",
    _fuchsia_virtual_device = "fuchsia_virtual_device",
)
load(
    "//fuchsia/private/workflows:fuchsia_task_flash.bzl",
    _fuchsia_task_flash = "fuchsia_task_flash",
)

# Rules
fuchsia_prebuilt_assembly_developer_overrides = _fuchsia_prebuilt_assembly_developer_overrides
fuchsia_assembly_developer_overrides_list = _fuchsia_assembly_developer_overrides_list
fuchsia_gen_android_starnix_container = _fuchsia_gen_android_starnix_container
fuchsia_platform_artifacts = _fuchsia_platform_artifacts
fuchsia_prebuilt_package = _fuchsia_prebuilt_package
fuchsia_package_directory = _fuchsia_package_directory
fuchsia_package_with_configs = _fuchsia_package_with_configs
fuchsia_product_configuration = _fuchsia_product_configuration
fuchsia_prebuilt_product_configuration = _fuchsia_prebuilt_product_configuration
fuchsia_prebuilt_product_configuration_extract_package = _fuchsia_prebuilt_product_configuration_extract_package
fuchsia_product_input_bundle = _fuchsia_product_input_bundle
fuchsia_prebuilt_product_input_bundle = _fuchsia_prebuilt_product_input_bundle
fuchsia_product_ota_config = _fuchsia_product_ota_config
fuchsia_virtual_device = _fuchsia_virtual_device
fuchsia_board_configuration = _fuchsia_board_configuration
fuchsia_board_input_bundle = _fuchsia_board_input_bundle
fuchsia_prebuilt_board_input_bundle = _fuchsia_prebuilt_board_input_bundle
fuchsia_board_input_bundle_set = _fuchsia_board_input_bundle_set
fuchsia_prebuilt_board_input_bundle_set = _fuchsia_prebuilt_board_input_bundle_set
fuchsia_prebuilt_board_configuration = _fuchsia_prebuilt_board_configuration
fuchsia_hybrid_board_configuration = _fuchsia_hybrid_board_configuration
fuchsia_hybrid_product_configuration = _fuchsia_hybrid_product_configuration
fuchsia_product = _fuchsia_product
fuchsia_partitions_configuration = _fuchsia_partitions_configuration
fuchsia_ssh_key_upload_method_inline = _fuchsia_ssh_key_upload_method_inline
fuchsia_ssh_key_upload_method_staged = _fuchsia_ssh_key_upload_method_staged
fuchsia_prebuilt_partitions_configuration = _fuchsia_prebuilt_partitions_configuration
fuchsia_product_bundle = _fuchsia_product_bundle
fuchsia_test_product_bundle = _fuchsia_test_product_bundle
fuchsia_product_size_check = _fuchsia_product_size_check
fuchsia_post_processing_script = _fuchsia_post_processing_script
fuchsia_package_size_check = _fuchsia_package_size_check
fuchsia_size_report_aggregator = _fuchsia_size_report_aggregator
fuchsia_elf_sizes = _fuchsia_elf_sizes
fuchsia_update_package = _fuchsia_update_package
fuchsia_repository_keys = _fuchsia_repository_keys
fuchsia_task_flash = _fuchsia_task_flash
fuchsia_scrutiny_config = _fuchsia_scrutiny_config
fuchsia_starnix_container = _fuchsia_starnix_container
fuchsia_starnix_file_create = _fuchsia_starnix_file_create
fuchsia_starnix_file_delete = _fuchsia_starnix_file_delete
fuchsia_starnix_file_override = _fuchsia_starnix_file_override

fuchsia_bootstrap_partition = _fuchsia_bootstrap_partition
fuchsia_bootloader_partition = _fuchsia_bootloader_partition
fuchsia_partition = _fuchsia_partition

# constants
BUILD_TYPES = _BUILD_TYPES
PARTITION_TYPE = _PARTITION_TYPE
INCLUDE_IN = _INCLUDE_IN
SLOT = _SLOT
ARCH = _ARCH
DEFAULT_PORTS = _DEFAULT_PORTS
INPUT_DEVICE_TYPE = _INPUT_DEVICE_TYPE
DELIVERY_BLOB_TYPE = _DELIVERY_BLOB_TYPE
SCRUTINY_VERIFIERS = _SCRUTINY_VERIFIERS

# Helper functions
ota_realm = _ota_realm
tuf_repo = _tuf_repo
tuf_repo_root = _tuf_repo_root
