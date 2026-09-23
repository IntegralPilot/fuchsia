# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

bringup_platform_aib_names = [
    # The kernel images by build type.
    "zircon_eng",
    "zircon_user",
    "zircon_userdebug",

    # The embeddable feature-set-level
    "embeddable",

    # component_manager is separate from embeddable so that
    # component_manager_with_tracing can be used instead.
    "component_manager",
    "component_manager_with_tracing",

    # The bootstrap feature-set-level
    "bootstrap",

    # Developer
    "kernel_debug_broker_user",

    # Diagnostics
    "console",
    "console-eng",

    # Driver Framework
    "driver_framework_common",
    "driver_framework_rust",
    "driver_framework_no_instrumentation",

    # Power
    "legacy_power_framework",
    "power_framework_eager_shard",
    "cpu_manager",
    "power_driver",
    "pwm_driver",

    # Graphics
    "display_drivers_boot",
    "input_drivers",
    "virtcon",

    # Kernel args
    "kernel_args_user",
    "kernel_contiguous_physical_pages",
    "kernel_logs_in_reboot_info",
    "kernel_arm64_event_stream_disable",

    # Static resource files.
    "resources",

    # Emulator Support
    "emulator_support",
    "vsock_service",

    # USB
    "usb_host_drivers",
    "usb_peripheral_drivers_boot",
    "usb_rndis_function",
    "usb_ums_function",

    # Storage
    "auto_slot_committer",
    "fshost_common",
    "fshost_non_eng",
    "fshost_non_recovery",
    "fshost_provision_fxfs",
    "fshost_storage",
    "fshost_fvm_minfs",
    "fshost_fxfs",
    "fshost_gpt_fvm_minfs",
    "fshost_recovery",
    "paver_shards_bootstrap",
    "paver_legacy",

    # SWD (Software Delivery)
    "no_update_checker",

    # Platform drivers.
    "registers_driver",
    "bt_transport_uart_driver",
    "bus_pci_driver",
    "realtek_8211f_driver",
    "xhci_driver",
    "sdhci_driver",
    "cqhci_driver",

    # The embeddable feature-set-level
    "embeddable_userdebug",

    # The bootstrap feature-set-level
    "bootstrap_userdebug",
    "clock_development_tools",

    # Connectivity
    "network_drivers_boot",
    "usb_cdc_function_boot",

    # Kernel args
    "kernel_args_eng",
    "kernel_args_userdebug",

    # Developer
    "bootstrap_realm_development_access",
    "bootstrap_realm_vsock_development_access",
    "kernel_debug_broker_userdebug",
    "netsvc",
    "ptysvc",

    # Emulator Support
    "vsock_service_bootstrap",

    # Power Framework
    "power_framework_broker",
    "power_framework_sag",

    # Trusted application support.
    "trusted_execution_environment",

    # Timekeeper
    "timekeeper_persistence",

    # Wake alarms support: generic and then hardware-specific.
    "timekeeper_wake_alarms",

    # Platform drivers.
    "interconnect_driver",

    # The embeddable feature-set-level
    "embeddable_eng",

    # The bootstrap feature-set-level
    "bootstrap_eng",

    # Kernel args
    "kernel_oom_reboot_timeout_low",
    "kernel_oom_behavior_jobkill",
    "kernel_oom_behavior_disable",
    "kernel_pmm_checker_enabled",
    "kernel_pmm_checker_enabled_auto",
    "kernel_serial_legacy",

    # Power Framework
    "power_framework_testing_sag",
    "power_test_platform_drivers",

    # Storage
    "fshost_eng",
    "fshost_gpt_fvm_f2fs",
    "partitioning_tools",

    # Testing Support
    "testing_support_bootstrap",

    # PCI utilities
    "lspci",

    # Platform drivers.
    "ufs_pci_driver",
    "ufs_pdev_driver",

    # UFS device user-space utility.
    "ufsutil",
]

platform_aib_names = bringup_platform_aib_names + [
    # The common platform bundles

    ## The core realm bundles

    # `/core` itself
    "core_realm",
    "core_realm_user_and_userdebug",

    # The additional children of core we add when we have networking enabled
    "core_realm_networking",
    "network_realm",
    "network_realm_packages",
    "network_realm_packages_gub",
    "network_tun",
    "thread_lowpan",
    "networking_with_virtualization",
    "networking_basic",
    "networking_basic_packages",
    "networking_basic_packages_gub",
    "mdns",

    # The minimal feature-set-level
    "common_standard",

    # Feature-level / Subsystem-level bundles
    # Keep sorted alphabetically.

    # Bluetooth
    "bluetooth_a2dp",
    "bluetooth_avrcp",
    "bluetooth_core",
    "bluetooth_device_id",
    "bluetooth_hfp_ag",
    "bluetooth_map_mce",
    "bluetooth_rfcomm",
    "bluetooth_snoop_eager",
    "bluetooth_snoop_lazy",

    # Media
    "audio_core",
    "audio_core_routing",
    "audio_core_use_adc_device",
    "audio_device_registry",
    "audio_device_registry_demand",
    "soundplayer",
    "camera",
    "media_codecs",
    "media_sessions",

    # Diagnostics
    "diagnostics_triage_detect_mali",
    "detect_user",

    # Fonts
    "fonts",
    "fonts_hermetic",

    # Connectivity
    "network_drivers_base",
    "usb_cdc_function_base",

    # Graphics
    "vulkan_loader",
    "lavapipe_pkg",
    "display_drivers_base",
    "fake_display_stack_host",

    # SWD (Software Delivery)
    "product_provided_update_checker",
    "omaha_client",
    "system_update_configurator",
    "swd_trust_store_restricted",
    "swd_trust_store_public",

    # Memory monitor
    "memory_monitor",
    "memory_monitor_page_refaults",

    # Netstack
    "netstack2",
    "netstack3",
    "netstack3_packages",
    "netstack3_packages_gub",
    "netstack_migration",
    "netstack_migration_packages",
    "netstack_migration_packages_gub",
    "socket-proxy-enabled",
    "socket-proxy-disabled",
    "socket_proxy_packages",

    # Location
    "location_emergency",

    # WLAN
    "wlan_legacy_privacy_support",
    "wlan_contemporary_privacy_only_support",
    "wlan_fullmac_support",
    "wlan_policy",
    "wlan_softmac_support",
    "wlan_virtio_hwsim_driver",
    "wlan_wlanix",

    # Sensors
    "sensors_framework",

    # Session
    "element_manager",
    "session_manager",
    "session_manager_disable_pkg_cache",

    # SetUI
    "setui",
    "setui_with_camera",

    # Storage
    "erofs_support",
    "factory_data",
    "paver_shards_core",
    "storage_cache_manager",

    # I18n
    "no_intl_timezones",

    # UI
    "ui",
    "ui_user_and_userdebug",
    "ui_userdebug_dso",
    "ui_package_user_and_userdebug",
    "ui_package_eng_userdebug_with_synthetic_device_support",
    "brightness_manager",
    "dso_runner",

    # Drivers
    "radar_proxy_without_injector",

    # Thermal
    "fan",

    # Battery
    "battery_manager",

    # Power metrics recorder
    "power_metrics_recorder",

    # Forensics
    "no_remote_feedback_id",
    "cobalt_user_config",

    # Kernel Reclamation
    "kernel_anonymous_memory_compression",
    "kernel_anonymous_memory_compression_eager_lru",
    "kernel_page_scanner_aging_fast",
    "kernel_page_table_eviction_never",
    "kernel_page_table_eviction_on_request",

    # USB
    "usb_peripheral_drivers_base",
    "usb_policy",
    "usb_policy_starnix",
    "usb_zero_function_base",

    # Recovery
    "factory_reset",
    "factory_reset_trigger",
    "factory_reset_no_tee",
    "factory_reset_tee",
    "recovery_fdr",

    # Starnix
    "starnix_support",

    # Virtualization
    "virtualization_support",

    # The tzif zoneinfo files
    "zoneinfo",

    # Security / Trusted Execution
    "tee_manager",
    "usb_adb_function",
    "bluetooth_hfp_hf",
    "core_realm_development_access",
    "core_realm_development_access_rcs_usb",
    "core_realm_development_access_userdebug",
    "hvdcp_opti_support",
    "session_manager_enable_pkg_cache",
    "standard_userdebug",
    "standard_userdebug_and_eng",
    "cobalt_userdebug_config",
    "mdns_fuchsia_device_wired_service",
    "nanohub_support",
    "fastrpc_support",
    "omaha_client_empty_eager_config",
    "radar_proxy_with_injector",
    "sl4f",
    "wlan_development",

    # Development and debug tools for connectivity
    "development_support_tools_connectivity_networking",
    "development_support_tools_connectivity_wlan",
    "development_support_tools_connectivity_thread",

    # Driver migration to Platform AIBs, but not needed in user builds.
    "fake_battery_driver",

    # Memory monitor
    "memory_monitor_with_memory_sampler",
    "memory_monitor_critical_reports",
    "memory_monitor2",

    # Development and debug tools for power framework
    "power_framework_development_support",

    # Development and debug tools for hrtimer
    "hrtimer-ctl",

    # Sensors support with playback.
    "sensors_framework_eng",

    # Tracing support.
    "tracing",

    # Recovery
    "recovery_android",

    # Userspace fastboot over usb support
    "fastbootd_usb_support",

    # Location
    "gnss",

    # the core realm additions for eng build-type assemblies
    "core_realm_eng",

    # SSH Config for eng only
    "core_realm_development_access_eng",

    # This isn't in all eng builds, but is in some,
    # and not in any non-eng builds.
    "component_manager_with_tracing_and_heapdump",

    # The minimal additions for eng build-type assemblies
    "standard_eng",

    # SWD (Software Delivery)
    "system_update_checker",
    "pkgfs_disable_executability_restrictions",

    # Testing Support
    "testing_support",

    # UI
    "ui_eng",
    "ui_package_eng",
    "ui_eng_dso",

    # Example AIB
    "example_assembly_bundle",

    # Topology test support
    "topology_test_daemon",

    # Driver development support
    "driver_framework_rust_with_heapdump",
    "driver_framework_with_heapdump",
    "full_drivers",

    # Driver support
    "base_drivers_storage",

    # Audio development/debugging
    "audio_development_support",
    "audio_device_registry_eager",
    "audio_driver_development_tools",
    "audio_full_stack_development_tools",
    "audio_legacy_driver_tools",
    "virtual_audio_driver",
    "virtual_audio_legacy_driver",
    "virtual_audio_util",
    "virtual_audio_legacy_util",
    "intel_hda",

    # Display development/debugging
    "display_driver_development_tools",

    # Video development/debugging
    "video_development_support",

    # Fake power sensor
    "fake_power_sensor",

    # Bluetooth testing support
    "bluetooth_a2dp_with_consumer",
    "bluetooth_affordances",
    "bluetooth_pandora",

    # Forensics
    "cobalt_default_config",

    # Memory profiling
    "heapdump_global_collector",
]

icu_platform_aib_base_names = [
    "setui",
    "setui_with_camera",
    "intl_services",
    "intl_services_small",
    "intl_services_small_with_timezone",
    "ui_userdebug_dso",
    "ui_user_and_userdebug",
    "ui_eng",
    "ui_eng_dso",
]
