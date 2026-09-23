// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use num_derive::{FromPrimitive, ToPrimitive};
use zerocopy::byteorder::little_endian::U32;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

const ZBI_MAX_SMT: usize = 4;

/// Must be in the extra field for headers of type ZbiType::Container.
pub const ZBI_CONTAINER_MAGIC: u32 = 0x868c_f7e6;

/// Every header type must have this magic value.
pub const ZBI_ITEM_MAGIC: u32 = 0xb578_1729;

/// Always required.
pub const ZBI_FLAGS_VERSION: u32 = 0x0001_0000;

/// If the header contains this flag, the CRC32 field must contain a valid CRC32. Otherwise, the
/// CRC32 field must contain ZBI_ITEM_NO_CRC32.
pub const ZBI_FLAGS_CRC32: u32 = 0x0002_0000;

/// The CRC32 field must be set to this when not using CRC32.
pub const ZBI_ITEM_NO_CRC32: u32 = 0x4a87_e8d6;

// Each item is padded to be a multiple of 8 bytes.
pub const ZBI_ALIGNMENT_BYTES: u32 = 0x8;

pub fn is_zbi_type_driver_metadata(zbi_type_raw: u32) -> bool {
    (zbi_type_raw & 0xFF) == ZbiType::DriverMetadata.into_raw()
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Defines the types supported by the Rust ZBI parser. This is a subset from
/// sdk/lib/zbi-format/include/lib/zbi-format/zbi.h, and should be updated as needed.
pub enum ZbiType {
    Container = 0x544f_4f42,            // 'BOOT'
    Cmdline = 0x4c44_4d43,              // 'CMDL'
    Crashlog = 0x4d4f_4f42,             // 'BOOM'
    KernelDriver = 0x5652_444B,         // 'KDRV'
    PlatformId = 0x4449_4C50,           // 'PLID'
    StorageBootfsFactory = 0x4653_4642, // 'BFSF'
    StorageRamdisk = 0x4b53_4452,       // 'RDSK'
    ImageArgs = 0x4752_4149,            // 'IARG'
    SerialNumber = 0x4e4c_5253,         // 'SRLN'
    BootloaderFile = 0x4C46_5442,       // 'BTFL'
    DeviceTree = 0xd00d_feed,           // ??
    CpuTopology = 0x544f_504f,          // 'TOPO'
    AcpiRsdp = 0x5044_5352,             // 'RSDP'
    Smbios = 0x4942_4d53,               // 'SMBI'
    Framebuffer = 0x4246_5753,          // 'SWFB'

    // DriverMetadata is a special case, where only the LSB of the u32 needs to match this
    // value. See the IsZbiTypeDriverMetadata function for details.
    DriverMetadata = 0x6D,
    Unknown,
}

impl ZbiType {
    pub fn from_raw(raw: u32) -> Self {
        match raw {
            x if x == ZbiType::Container.into_raw() => ZbiType::Container,
            x if x == ZbiType::Cmdline.into_raw() => ZbiType::Cmdline,
            x if x == ZbiType::Crashlog.into_raw() => ZbiType::Crashlog,
            x if x == ZbiType::KernelDriver.into_raw() => ZbiType::KernelDriver,
            x if x == ZbiType::PlatformId.into_raw() => ZbiType::PlatformId,
            x if x == ZbiType::StorageBootfsFactory.into_raw() => ZbiType::StorageBootfsFactory,
            x if x == ZbiType::StorageRamdisk.into_raw() => ZbiType::StorageRamdisk,
            x if x == ZbiType::ImageArgs.into_raw() => ZbiType::ImageArgs,
            x if x == ZbiType::SerialNumber.into_raw() => ZbiType::SerialNumber,
            x if x == ZbiType::BootloaderFile.into_raw() => ZbiType::BootloaderFile,
            x if x == ZbiType::DeviceTree.into_raw() => ZbiType::DeviceTree,
            x if x == ZbiType::CpuTopology.into_raw() => ZbiType::CpuTopology,
            x if x == ZbiType::AcpiRsdp.into_raw() => ZbiType::AcpiRsdp,
            x if x == ZbiType::Smbios.into_raw() => ZbiType::Smbios,
            x if x == ZbiType::Framebuffer.into_raw() => ZbiType::Framebuffer,
            x if is_zbi_type_driver_metadata(x) => ZbiType::DriverMetadata,
            _ => ZbiType::Unknown,
        }
    }

    pub fn into_raw(self) -> u32 {
        self as u32
    }
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone, KnownLayout, FromBytes, IntoBytes, Immutable, Unaligned)]
pub struct zbi_header_t {
    pub zbi_type: U32,
    pub length: U32,
    pub extra: U32,
    pub flags: U32,
    pub reserved_0: U32,
    pub reserved_1: U32,
    pub magic: U32,
    pub crc32: U32,
}

/// Define a container header that describes a container content length of `length`.
pub fn zbi_container_header(length: u32) -> zbi_header_t {
    zbi_header_t {
        zbi_type: U32::new(ZbiType::Container.into_raw()),
        length: U32::new(length),
        extra: U32::new(ZBI_CONTAINER_MAGIC),
        flags: U32::new(ZBI_FLAGS_VERSION),
        reserved_0: U32::new(0),
        reserved_1: U32::new(0),
        magic: U32::new(ZBI_ITEM_MAGIC),
        crc32: U32::new(ZBI_ITEM_NO_CRC32),
    }
}

#[repr(C)]
#[derive(Copy, Clone, KnownLayout, FromBytes, Immutable)]
/// Defines the Rust version of `zbi_topology_node_v2_t` in
/// sdk/lib/zbi-format/include/lib/zbi-format/zbi.h.
pub struct ZbiTopologyNode {
    // Should be one of ZbiTopologyEntityType.
    pub entity_type: u8,
    pub parent_index: u16,
    pub entity: Entity,
}

#[repr(u8)]
#[derive(FromPrimitive, ToPrimitive)]
pub enum ZbiTopologyEntityType {
    ZbiTopologyEntityUndefined = 0,
    ZbiTopologyEntityProcessor = 1,
    ZbiTopologyEntityCluster = 2,
    ZbiTopologyEntityCache = 3,
    ZbiTopologyEntityDie = 4,
    ZbiTopologyEntitySocket = 5,
    ZbiTopologyEntityPowerPlane = 6,
    ZbiTopologyEntityNumaRegion = 7,
}

#[repr(u8)]
#[derive(FromPrimitive, ToPrimitive)]
pub enum ZbiTopologyArchitecture {
    ZbiTopologyArchUndefined = 0,
    ZbiTopologyArchX64 = 1,
    ZbiTopologyArchArm64 = 2,
}

#[repr(C)]
#[derive(Copy, Clone, KnownLayout, FromBytes, Immutable)]
pub union Entity {
    pub processor: ZbiTopologyProcessor,
    pub cluster: ZbiTopologyCluster,
    pub numa_region: ZbiTopologyNumaRegion,
    pub cache: ZbiTopologyCache,
}

#[repr(C)]
#[derive(Copy, Clone, KnownLayout, FromBytes, Immutable)]
pub struct ZbiTopologyProcessor {
    pub logical_ids: [u16; ZBI_MAX_SMT],
    pub logical_id_count: u8,
    pub flags: u16,

    // Should be one of ZbiTopologyArchitecture.
    // If UNDEFINED then nothing will be set in arch_info.
    pub architecture: u8,
    pub architecture_info: ArchitectureInfo,
}

#[repr(C)]
#[derive(Copy, Clone, KnownLayout, FromBytes, Immutable)]
pub union ArchitectureInfo {
    pub arm64: ZbiTopologyArm64Info,
    pub x64: ZbiTopologyX64Info,
}

#[repr(C)]
#[derive(Copy, Clone, KnownLayout, FromBytes, IntoBytes, Immutable)]
pub struct ZbiTopologyCluster {
    // Relative performance level of this processor in the system.
    // Refer to sdk/lib/zbi-format/include/lib/zbi-format/zbi.h for more details.
    pub performance_class: u8,
}

#[repr(C)]
#[derive(Copy, Clone, KnownLayout, FromBytes, IntoBytes, Immutable)]
pub struct ZbiTopologyNumaRegion {
    // Starting and ending memory addresses of this numa region.
    pub start_address: u64,
    pub end_address: u64,
}

#[repr(C)]
#[derive(Copy, Clone, KnownLayout, FromBytes, IntoBytes, Immutable)]
pub struct ZbiTopologyCache {
    // Unique id of this cache node. No other semantics are assumed.
    pub cache_id: u32,
}

#[repr(C)]
#[derive(Copy, Clone, KnownLayout, FromBytes, IntoBytes, Immutable)]
pub struct ZbiTopologyArm64Info {
    // Cluster ids for each level, one being closest to the cpu.
    // These map to aff1, aff2, and aff3 values in the ARM registers.
    pub cluster_1_id: u8,
    pub cluster_2_id: u8,
    pub cluster_3_id: u8,

    // Id of the cpu inside of the bottom-most cluster, aff0 value.
    pub cpu_id: u8,

    // The GIC interface number for this processor.
    // In GIC v3+ this is not necessary as the processors are addressed by their
    // affinity routing (all cluster ids followed by cpu_id).
    pub gic_id: u8,
}

#[repr(C)]
#[derive(Copy, Clone, KnownLayout, FromBytes, IntoBytes, Immutable)]
pub struct ZbiTopologyX64Info {
    // Indexes here correspond to the logical_ids index for the thread.
    pub apic_ids: [u32; ZBI_MAX_SMT],
    pub apic_id_count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX_SERIALIZATION_BUFFER_SIZE: usize = 128;

    // From libtest-lib.a
    extern "C" {
        pub fn serialize_zbi_topology_x64_info_t(
            buffer: &[u8; MAX_SERIALIZATION_BUFFER_SIZE],
            apic_ids: &[u32; 4],
            apic_id_count: u32,
        ) -> usize;

        pub fn serialize_zbi_topology_arm64_info_t(
            buffer: &[u8; MAX_SERIALIZATION_BUFFER_SIZE],
            cluster_1_id: u8,
            cluster_2_id: u8,
            cluster_3_id: u8,
            cpu_id: u8,
            gic_id: u8,
        ) -> usize;

        pub fn serialize_zbi_topology_cache_t(
            buffer: &[u8; MAX_SERIALIZATION_BUFFER_SIZE],
            cache_id: u32,
        ) -> usize;

        pub fn serialize_zbi_topology_numa_region_v2_t(
            buffer: &[u8; MAX_SERIALIZATION_BUFFER_SIZE],
            start_address: u64,
            end_address: u64,
        ) -> usize;

        pub fn serialize_zbi_topology_cluster_t(
            buffer: &[u8; MAX_SERIALIZATION_BUFFER_SIZE],
            performance_class: u8,
        ) -> usize;

        pub fn serialize_zbi_topology_processor_v2_t(
            buffer: &[u8; MAX_SERIALIZATION_BUFFER_SIZE],
            logical_ids: &[u16; 4],
            logical_id_count: u8,
            flags: u16,
            architecture: u8,
            architecture_info: ArchitectureInfo,
        ) -> usize;

        pub fn serialize_zbi_topology_node_v2_t(
            buffer: &[u8; MAX_SERIALIZATION_BUFFER_SIZE],
            entity_type: u8,
            parent_index: u16,
            entity: Entity,
        ) -> usize;
    }

    unsafe fn as_u8_slice<T: Sized>(p: &T) -> &[u8] {
        std::slice::from_raw_parts((p as *const T) as *const u8, ::std::mem::size_of::<T>())
    }

    #[fuchsia::test]
    fn zbi_topology_x64_info_in_sync() {
        let apic_ids = [0, 1, 3, 4];
        let apic_id_count = 4;

        let buffer = [0 as u8; MAX_SERIALIZATION_BUFFER_SIZE];
        let size = unsafe { serialize_zbi_topology_x64_info_t(&buffer, &apic_ids, apic_id_count) };
        assert_eq!(size, std::mem::size_of::<ZbiTopologyX64Info>());

        let x64_info = ZbiTopologyX64Info { apic_ids, apic_id_count };
        assert_eq!(x64_info.as_bytes(), &buffer[0..size]);
    }

    #[fuchsia::test]
    fn zbi_topology_arm_info_in_sync() {
        let cluster_1_id = 1;
        let cluster_2_id = 2;
        let cluster_3_id = 3;
        let cpu_id = 0;
        let gic_id = 1;

        let buffer = [0 as u8; MAX_SERIALIZATION_BUFFER_SIZE];
        let size = unsafe {
            serialize_zbi_topology_arm64_info_t(
                &buffer,
                cluster_1_id,
                cluster_2_id,
                cluster_3_id,
                cpu_id,
                gic_id,
            )
        };
        assert_eq!(size, std::mem::size_of::<ZbiTopologyArm64Info>());

        let arm_info =
            ZbiTopologyArm64Info { cluster_1_id, cluster_2_id, cluster_3_id, cpu_id, gic_id };
        assert_eq!(arm_info.as_bytes(), &buffer[0..size]);
    }

    #[fuchsia::test]
    fn zbi_topology_cache_in_sync() {
        let cache_id = 1221;

        let buffer = [0 as u8; MAX_SERIALIZATION_BUFFER_SIZE];
        let size = unsafe { serialize_zbi_topology_cache_t(&buffer, cache_id) };
        assert_eq!(size, std::mem::size_of::<ZbiTopologyCache>());

        let cache = ZbiTopologyCache { cache_id };
        assert_eq!(cache.as_bytes(), &buffer[0..size]);
    }

    #[fuchsia::test]
    fn zbi_topology_numa_region_in_sync() {
        let start_address = 1221;
        let end_address = 1223;

        let buffer = [0 as u8; MAX_SERIALIZATION_BUFFER_SIZE];
        let size =
            unsafe { serialize_zbi_topology_numa_region_v2_t(&buffer, start_address, end_address) };
        assert_eq!(size, std::mem::size_of::<ZbiTopologyNumaRegion>());

        let numa_region = ZbiTopologyNumaRegion { start_address, end_address };
        assert_eq!(numa_region.as_bytes(), &buffer[0..size]);
    }

    #[fuchsia::test]
    fn zbi_topology_cluster_in_sync() {
        let performance_class = 2;

        let buffer = [0 as u8; MAX_SERIALIZATION_BUFFER_SIZE];
        let size = unsafe { serialize_zbi_topology_cluster_t(&buffer, performance_class) };
        assert_eq!(size, std::mem::size_of::<ZbiTopologyCluster>());

        let cluster = ZbiTopologyCluster { performance_class };
        assert_eq!(cluster.as_bytes(), &buffer[0..size]);
    }

    #[fuchsia::test]
    fn zbi_topology_processor_in_sync() {
        let logical_ids = [0, 1, 3, 4];
        let logical_id_count = 4;
        let flags = 1;
        let arm_info = ZbiTopologyArm64Info {
            cluster_1_id: 1,
            cluster_2_id: 1,
            cluster_3_id: 0,
            cpu_id: 0,
            gic_id: 1,
        };
        let x64_info = ZbiTopologyX64Info { apic_ids: [0, 1, 3, 4], apic_id_count: 4 };
        let mut architecture_info = ArchitectureInfo { arm64: arm_info };

        let mut buffer = [0 as u8; MAX_SERIALIZATION_BUFFER_SIZE];
        let mut size = unsafe {
            serialize_zbi_topology_processor_v2_t(
                &buffer,
                &logical_ids,
                logical_id_count,
                flags,
                ZbiTopologyArchitecture::ZbiTopologyArchArm64 as u8,
                architecture_info,
            )
        };
        assert_eq!(size, std::mem::size_of::<ZbiTopologyProcessor>());

        // Ensure that the padding introduced by union are all zeroed.
        let mut arm_processor: ZbiTopologyProcessor = unsafe { std::mem::zeroed() };
        arm_processor.logical_ids = logical_ids;
        arm_processor.logical_id_count = logical_id_count;
        arm_processor.flags = flags;
        arm_processor.architecture = ZbiTopologyArchitecture::ZbiTopologyArchArm64 as u8;
        arm_processor.architecture_info.arm64 = arm_info;
        assert_eq!(unsafe { as_u8_slice(&arm_processor) }, &buffer[0..size]);

        buffer = [0 as u8; MAX_SERIALIZATION_BUFFER_SIZE];
        architecture_info = ArchitectureInfo { x64: x64_info };
        size = unsafe {
            serialize_zbi_topology_processor_v2_t(
                &buffer,
                &logical_ids,
                logical_id_count,
                flags,
                ZbiTopologyArchitecture::ZbiTopologyArchX64 as u8,
                architecture_info,
            )
        };
        assert_eq!(size, std::mem::size_of::<ZbiTopologyProcessor>());

        // Ensure that the padding introduced by union are all zeroed.
        let mut x64_processor: ZbiTopologyProcessor = unsafe { std::mem::zeroed() };
        x64_processor.logical_ids = logical_ids;
        x64_processor.logical_id_count = logical_id_count;
        x64_processor.flags = flags;
        x64_processor.architecture = ZbiTopologyArchitecture::ZbiTopologyArchX64 as u8;
        x64_processor.architecture_info.x64 = x64_info;
        assert_eq!(unsafe { as_u8_slice(&x64_processor) }, &buffer[0..size]);

        buffer = [0 as u8; MAX_SERIALIZATION_BUFFER_SIZE];
        size = unsafe {
            serialize_zbi_topology_processor_v2_t(
                &buffer,
                &logical_ids,
                logical_id_count,
                flags,
                ZbiTopologyArchitecture::ZbiTopologyArchUndefined as u8,
                architecture_info,
            )
        };
        assert_eq!(size, std::mem::size_of::<ZbiTopologyProcessor>());

        // Ensure that the padding introduced by union are all zeroed.
        let mut undefined_processor: ZbiTopologyProcessor = unsafe { std::mem::zeroed() };
        undefined_processor.logical_ids = logical_ids;
        undefined_processor.logical_id_count = logical_id_count;
        undefined_processor.flags = flags;
        undefined_processor.architecture = ZbiTopologyArchitecture::ZbiTopologyArchUndefined as u8;
        assert_eq!(unsafe { as_u8_slice(&undefined_processor) }, &buffer[0..size]);
    }

    #[fuchsia::test]
    fn zbi_topology_node_in_sync() {
        let parent_index = 5;
        // Ensure that the padding introduced by union are all zeroed.
        let mut processor: ZbiTopologyProcessor = unsafe { std::mem::zeroed() };
        processor.logical_ids = [0, 0, 3, 0];
        processor.logical_id_count = 4;
        processor.flags = 2;
        processor.architecture = ZbiTopologyArchitecture::ZbiTopologyArchUndefined as u8;
        let mut entity = Entity { processor: processor };

        let mut buffer = [0 as u8; MAX_SERIALIZATION_BUFFER_SIZE];
        let mut size = unsafe {
            serialize_zbi_topology_node_v2_t(
                &buffer,
                ZbiTopologyEntityType::ZbiTopologyEntityProcessor as u8,
                parent_index,
                entity,
            )
        };
        assert_eq!(size, std::mem::size_of::<ZbiTopologyNode>());

        // Ensure that the padding introduced by union are all zeroed.
        let mut processor_node: ZbiTopologyNode = unsafe { std::mem::zeroed() };
        processor_node.parent_index = parent_index;
        processor_node.entity_type = ZbiTopologyEntityType::ZbiTopologyEntityProcessor as u8;
        processor_node.entity.processor = processor;
        assert_eq!(unsafe { as_u8_slice(&processor_node) }, &buffer[0..size]);

        let cluster = ZbiTopologyCluster { performance_class: 1 };
        entity = Entity { cluster: cluster };
        buffer = [0 as u8; MAX_SERIALIZATION_BUFFER_SIZE];
        size = unsafe {
            serialize_zbi_topology_node_v2_t(
                &buffer,
                ZbiTopologyEntityType::ZbiTopologyEntityCluster as u8,
                parent_index,
                entity,
            )
        };
        assert_eq!(size, std::mem::size_of::<ZbiTopologyNode>());

        // Ensure that the padding introduced by union are all zeroed.
        let mut cluster_node: ZbiTopologyNode = unsafe { std::mem::zeroed() };
        cluster_node.parent_index = parent_index;
        cluster_node.entity_type = ZbiTopologyEntityType::ZbiTopologyEntityCluster as u8;
        cluster_node.entity.cluster = cluster;
        assert_eq!(unsafe { as_u8_slice(&cluster_node) }, &buffer[0..size]);

        let numa_region = ZbiTopologyNumaRegion { start_address: 2233, end_address: 3333 };
        entity = Entity { numa_region: numa_region };
        buffer = [0 as u8; MAX_SERIALIZATION_BUFFER_SIZE];
        size = unsafe {
            serialize_zbi_topology_node_v2_t(
                &buffer,
                ZbiTopologyEntityType::ZbiTopologyEntityNumaRegion as u8,
                parent_index,
                entity,
            )
        };
        assert_eq!(size, std::mem::size_of::<ZbiTopologyNode>());

        // Ensure that the padding introduced by union are all zeroed.
        let mut numa_region_node: ZbiTopologyNode = unsafe { std::mem::zeroed() };
        numa_region_node.parent_index = parent_index;
        numa_region_node.entity_type = ZbiTopologyEntityType::ZbiTopologyEntityNumaRegion as u8;
        numa_region_node.entity.numa_region = numa_region;
        assert_eq!(unsafe { as_u8_slice(&numa_region_node) }, &buffer[0..size]);

        let cache = ZbiTopologyCache { cache_id: 3 };
        entity = Entity { cache: cache };
        buffer = [0 as u8; MAX_SERIALIZATION_BUFFER_SIZE];
        size = unsafe {
            serialize_zbi_topology_node_v2_t(
                &buffer,
                ZbiTopologyEntityType::ZbiTopologyEntityCache as u8,
                parent_index,
                entity,
            )
        };
        assert_eq!(size, std::mem::size_of::<ZbiTopologyNode>());

        // Ensure that the padding introduced by union are all zeroed.
        let mut cache_node: ZbiTopologyNode = unsafe { std::mem::zeroed() };
        cache_node.parent_index = parent_index;
        cache_node.entity_type = ZbiTopologyEntityType::ZbiTopologyEntityCache as u8;
        cache_node.entity.cache = cache;
        assert_eq!(unsafe { as_u8_slice(&cache_node) }, &buffer[0..size]);
    }
}
