// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assembly_container::WalkPaths;
use camino::Utf8PathBuf;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Amount of memory.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemorySize {
    /// Number of bytes.
    Fixed(u64),
    /// Percentage of the system's physical memory.
    /// the value must be stritly positive, and less than 100.
    Percent(u8),
}

/// Platform configuration options for sysmem.
///
/// This config exists in both board and platform configs, to allow board config
/// to override static defaults, and to allow platform config to override board
/// config.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema, WalkPaths)]
#[serde(default, deny_unknown_fields)]
pub struct PlatformSysmemConfig {
    /// Overrides the board-driver-specified size for sysmem's contiguous memory
    /// pool. The default value is 5% if not set by board or platform config.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contiguous_memory_size: Option<MemorySize>,

    /// Overrides the board-driver-specified size for sysmem's default protected
    /// memory pool. The default value is 0 if not set by board or platform
    /// config.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protected_memory_size: Option<MemorySize>,

    /// If true, sysmem will treat a fraction of currently-unused pages as guard
    /// pages and attempt to loan the rest back to zircon. If false, sysmem will
    /// attempt to loan all currently-unused pages back to zircon.
    ///
    /// Enabling this will enable periodic timers in sysmem which check unused
    /// pages for stray DMA writes. The default is false to avoid the periodic
    /// timers by default. When true, on detection of an improperly written
    /// page, sysmem will attempt to log debug info re. allocations that
    /// previously used the page.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub contiguous_guard_pages_unused: bool,

    /// Optional ordered list of files, each of which contains a persistent fidl
    /// ['fuchsia.sysmem2.FormatCosts'].
    ///
    /// Normally json[5] would be preferable for config, but we generate this
    /// config in rust using FIDL types (to avoid repetition and to take
    /// advantage of FIDL rust codegen), and there's no json schema for FIDL
    /// types.
    ///
    /// In board_config::PlatformSettings.sysmem_defaults, this field must be None
    /// (see BoardProvidedConfig.sysmem_format_costs instead). In
    /// platform_settings::PlatformSettings.sysmem, this field can be Some.
    ///
    /// A later entry with equal FormatCostKey will override an earlier entry
    /// (both within a single file and across files). Entries in
    /// platform_settings::PlatformSettings.sysmem field are logically after entries
    /// in BoardProvidedConfig.sysmem_format_costs field.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[walk_paths]
    #[schemars(schema_with = "crate::vec_path_schema")]
    pub format_costs: Vec<Utf8PathBuf>,
}

/// Board configuration options for sysmem.
///
/// The settings in this struct can be overridden by settings in
/// PlatformSysmemConfig. See also BoardProvidedConfig.sysmem_format_costs which
/// can also be specified for the board.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct BoardSysmemConfig {
    /// Overrides the board-driver-specified size for sysmem's contiguous memory
    /// pool. The default value is 5% if not set by board or platform config.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contiguous_memory_size: Option<MemorySize>,

    /// Overrides the board-driver-specified size for sysmem's default protected
    /// memory pool. The default value is 0 if not set by board or platform
    /// config.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protected_memory_size: Option<MemorySize>,

    /// If true, sysmem will treat a fraction of currently-unused pages as guard
    /// pages and attempt to loan the rest back to zircon. If false, sysmem will
    /// attempt to loan all currently-unused pages back to zircon.
    ///
    /// Enabling this will enable periodic timers in sysmem which check unused
    /// pages for stray DMA writes. The default is false to avoid the periodic
    /// timers by default. When true, on detection of an improperly written
    /// page, sysmem will attempt to log debug info re. allocations that
    /// previously used the page.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub contiguous_guard_pages_unused: bool,
}
