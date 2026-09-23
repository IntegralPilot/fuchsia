# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

load("@fuchsia_build_info//:args.bzl", "compilation_mode")

BASE_PROVIDED_FEATURES = [
    "fuchsia::hrtimer",
    "fuchsia::paver",
    "fuchsia::pmm_checker",
    "fuchsia::power",
    "fuchsia::pwm",
    "fuchsia::shared_registers",
    "fuchsia::real_time_clock",
    "fuchsia::realtek_8211f",
    "fuchsia::storage_power_management",
    "fuchsia::usb_host",
    "fuchsia::usb_peripheral_support",
    "fuchsia::xhci",
]

MEDIA_PROVIDED_FEATURES = [
    "fuchsia::bt_transport_uart",
    "fuchsia::fake_battery",
    "fuchsia::fake_power_sensor",
    "fuchsia::fan",
    "fuchsia::input",
    "fuchsia::mali_gpu",
    "fuchsia::suspender",
    "fuchsia::suspending_token",
    "fuchsia::vulkan_gpu",
    "fuchsia::wlan_fullmac",
]

COMMON_FILESYSTEMS = {
    "vbmeta": {
        "key": "LABEL(//src/firmware/avb_keys/vim3/vim3-dev-key:vim3_devkey_atx_psk.pem)",
        "key_metadata": "LABEL(//src/firmware/avb_keys/vim3/vim3-dev-key:vim3_dev_atx_metadata.bin)",
    },
    "zbi": {
        "compression": "zstd.16" if compilation_mode == "debug" else "zstd",
    },
    "fvm": {
        "blobfs": {
            "size_checker_maximum_bytes": 5216665600,
        },
        "sparse_output": {
        },
        "fastboot_output": {
            # For VIM3, FVM partition uses all of the remaining eMMC.
            # However, the total size of the eMMC storage maybe 16G or 32G
            # depending on whether it is a basic or pro version. In
            # addition, the actual size of the user block allocated by
            # Fuchsia can be further different. (i.e. 'lsblk' shows a 29G
            # size user block for the 32Gb version). To avoid the risk of
            # overflowing available size, here we set it to be the same as
            # sherlock (3280mb), which is clearly safe and sufficient for
            # now.
            "truncate_to_length": 3439329280,
        },
    },
    "fxfs": {
        "size_checker_maximum_bytes": 5216665600,
    },
}

COMMON_PLATFORM = {
    "connectivity": {
        "network": {
            # Prefer using the built-in NIC to the CDC-ether interface.
            "netsvc_interface": "/dwmac-ff3f0000/dwmac/Designware-MAC/network-device",
        },
    },
    "development_support": {
        # Enable the Debug Access Port (DAP) for improved lockup/crash diagnostics.
        "enable_debug_access_port_for_soc": "amlogic-a311d",
    },
    "sysmem_defaults": {
        # The AMlogic display engine needs contiguous physical memory for each
        # frame buffer, because it does not have a page table walker.
        #
        # The maximum supported resolution is documented below.
        # * "A311D Quick Reference Manual" revision 01, pages 2-3
        # * "A311D Datasheet" revision 08, section 2.2 "Features", pages 4-5
        #
        # These pages can be loaned back to zircon for use in pager-backed VMOs,
        # but these pages won't be used in "anonymous" VMOs (at least for now).
        # Whether the loaned-back pages can be absorbed by pager-backed VMOs is
        # workload dependent. The "k ppb stats_on" command can be used to
        # determine whether all loaned pages are being used by pager-backed VMOs.
        #
        # This board-level default can be overridden by platform-level config.
        "contiguous_memory_size": {
            # 200 MiB
            "fixed": 209715200,
        },
        "protected_memory_size": {
            "fixed": 0,
        },
        "contiguous_guard_pages_unused": False,
    },
}
