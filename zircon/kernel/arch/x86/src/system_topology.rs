// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use debug::dprintf;
use zx_status::Status;

const LOCAL_TRACE: u32 = 0;

// TODO(edcoyne): move to fbl::Vector::resize().
fn grow_vector<T: Default>(vector: &mut fbl::Vector<T>, new_size: usize) -> Result<(), Status> {
    while vector.len() < new_size {
        vector.push_back(T::default()).map_err(|_| Status::NO_MEMORY)?;
    }
    Ok(())
}

struct Core {
    node: zbi::TopologyNode,
}

impl Core {
    fn new() -> Self {
        Self {
            node: zbi::TopologyNode {
                entity: zbi::TopologyEntity::Processor(zbi::TopologyProcessor {
                    architecture_info: zbi::TopologyArchitectureInfo::X64(zbi::TopologyX64Info {
                        apic_ids: [0; 4],
                        apic_id_count: 0,
                    }),
                    flags: zbi::TopologyProcessorFlags::empty(),
                    logical_ids: [0; 4],
                    logical_id_count: 0,
                }),
                parent_index: zbi::TOPOLOGY_NO_PARENT,
            },
        }
    }

    fn set_primary(&mut self, primary: bool) {
        if let zbi::TopologyEntity::Processor(ref mut processor) = self.node.entity {
            processor.flags = if primary {
                zbi::TopologyProcessorFlags::PRIMARY
            } else {
                zbi::TopologyProcessorFlags::empty()
            };
        }
    }

    fn add_thread(&mut self, logical_id: u16, apic_id: u32) {
        if let zbi::TopologyEntity::Processor(ref mut processor) = self.node.entity {
            let count = processor.logical_id_count as usize;
            assert!(count < 4);
            processor.logical_ids[count] = logical_id;
            processor.logical_id_count += 1;

            if let zbi::TopologyArchitectureInfo::X64(ref mut x64) = processor.architecture_info {
                let apic_count = x64.apic_id_count as usize;
                assert!(apic_count < 4);
                x64.apic_ids[apic_count] = apic_id;
                x64.apic_id_count += 1;
            }
        }
    }

    fn set_flat_parent(&mut self, parent_index: u16) {
        self.node.parent_index = parent_index;
    }

    fn node(&self) -> &zbi::TopologyNode {
        &self.node
    }
}

struct SharedCache {
    node: zbi::TopologyNode,
    cores: fbl::Vector<Option<kalloc::Box<Core>>>,
}

impl SharedCache {
    fn new(id: u32) -> Self {
        Self {
            node: zbi::TopologyNode {
                entity: zbi::TopologyEntity::Cache(zbi::TopologyCache { cache_id: id }),
                parent_index: zbi::TOPOLOGY_NO_PARENT,
            },
            cores: fbl::Vector::new(),
        }
    }

    fn get_core(&mut self, index: usize) -> Result<&mut Core, Status> {
        grow_vector(&mut self.cores, index + 1)?;
        if self.cores[index].is_none() {
            let core = kalloc::Box::try_new(Core::new()).map_err(|_| Status::NO_MEMORY)?;
            self.cores[index] = Some(core);
        }
        Ok(self.cores[index].as_mut().unwrap())
    }

    fn set_flat_parent(&mut self, parent_index: u16) {
        self.node.parent_index = parent_index;
    }

    fn node(&self) -> &zbi::TopologyNode {
        &self.node
    }

    fn cores(&mut self) -> &mut fbl::Vector<Option<kalloc::Box<Core>>> {
        &mut self.cores
    }
}

struct Die {
    node: zbi::TopologyNode,
    caches: fbl::Vector<Option<kalloc::Box<SharedCache>>>,
    cores: fbl::Vector<Option<kalloc::Box<Core>>>,
    numa: Option<acpi_lite::AcpiNumaDomain>,
}

impl Die {
    fn new() -> Self {
        Self {
            node: zbi::TopologyNode {
                entity: zbi::TopologyEntity::Die(zbi::TopologyDie { reserved: 0 }),
                parent_index: zbi::TOPOLOGY_NO_PARENT,
            },
            caches: fbl::Vector::new(),
            cores: fbl::Vector::new(),
            numa: None,
        }
    }

    fn get_cache(&mut self, index: usize) -> Result<&mut SharedCache, Status> {
        grow_vector(&mut self.caches, index + 1)?;
        if self.caches[index].is_none() {
            let cache = kalloc::Box::try_new(SharedCache::new(index as u32))
                .map_err(|_| Status::NO_MEMORY)?;
            self.caches[index] = Some(cache);
        }
        Ok(self.caches[index].as_mut().unwrap())
    }

    fn get_core(&mut self, index: usize) -> Result<&mut Core, Status> {
        grow_vector(&mut self.cores, index + 1)?;
        if self.cores[index].is_none() {
            let core = kalloc::Box::try_new(Core::new()).map_err(|_| Status::NO_MEMORY)?;
            self.cores[index] = Some(core);
        }
        Ok(self.cores[index].as_mut().unwrap())
    }

    fn set_flat_parent(&mut self, parent_index: u16) {
        self.node.parent_index = parent_index;
    }

    fn node(&self) -> &zbi::TopologyNode {
        &self.node
    }

    fn caches(&mut self) -> &mut fbl::Vector<Option<kalloc::Box<SharedCache>>> {
        &mut self.caches
    }

    fn cores(&mut self) -> &mut fbl::Vector<Option<kalloc::Box<Core>>> {
        &mut self.cores
    }

    fn set_numa(&mut self, numa: acpi_lite::AcpiNumaDomain) {
        self.numa = Some(numa);
    }

    fn numa(&self) -> Option<&acpi_lite::AcpiNumaDomain> {
        self.numa.as_ref()
    }
}

// Unlike the other topological levels, `Package`(/socket) does not define an
// explicit node in the synthesized topology; it serves here merely as a means
// of organizing dies.
struct Package {
    dies: fbl::Vector<Option<kalloc::Box<Die>>>,
}

impl Package {
    fn new() -> Self {
        Self { dies: fbl::Vector::new() }
    }

    fn get_die(&mut self, index: usize) -> Result<&mut Die, Status> {
        grow_vector(&mut self.dies, index + 1)?;
        if self.dies[index].is_none() {
            let die = kalloc::Box::try_new(Die::new()).map_err(|_| Status::NO_MEMORY)?;
            self.dies[index] = Some(die);
        }
        Ok(self.dies[index].as_mut().unwrap())
    }

    fn dies(&mut self) -> &mut fbl::Vector<Option<kalloc::Box<Die>>> {
        &mut self.dies
    }
}

struct PackageList<'a> {
    packages: fbl::Vector<Option<kalloc::Box<Package>>>,
    decoder: &'a libarch::x86::ApicIdDecoder,
    last_level_cache_id_shift: Option<usize>,
    // APIC ID of this processor, we will ensure it has logical_id 0;
    primary_apic_id: u32,
    next_logical_id: u16,
}

impl<'a> PackageList<'a> {
    fn new(
        decoder: &'a libarch::x86::ApicIdDecoder,
        primary_apic_id: u32,
        last_level_cache_id_shift: Option<usize>,
    ) -> Self {
        if last_level_cache_id_shift.is_none() {
            dprintf!(CRITICAL, "WARNING: could not determine LLC share ID shift\n");
        }
        Self {
            packages: fbl::Vector::new(),
            decoder,
            last_level_cache_id_shift,
            primary_apic_id,
            next_logical_id: 1,
        }
    }

    fn add(&mut self, entry: &acpi_lite::structures::AcpiMadtLocalApicEntry) -> Result<(), Status> {
        let apic_id = entry.apic_id as u32;
        let is_primary = self.primary_apic_id == apic_id;

        let pkg_id = self.decoder.package_id(apic_id) as usize;
        let die_id = self.decoder.die_id(apic_id) as usize;
        let core_id = self.decoder.core_id(apic_id) as usize;
        let smt_id = self.decoder.smt_id(apic_id);
        let cache_id = self.last_level_cache_id_shift.map(|shift| apic_id >> shift);

        grow_vector(&mut self.packages, pkg_id + 1)?;
        if self.packages[pkg_id].is_none() {
            let pkg = kalloc::Box::try_new(Package::new()).map_err(|_| Status::NO_MEMORY)?;
            self.packages[pkg_id] = Some(pkg);
        }

        let pkg = self.packages[pkg_id].as_mut().unwrap();
        let die = pkg.get_die(die_id)?;

        let core = if let Some(cache_id_val) = cache_id {
            let cache = die.get_cache(cache_id_val as usize)?;
            cache.get_core(core_id)?
        } else {
            die.get_core(core_id)?
        };

        let logical_id = if is_primary {
            0
        } else {
            let id = self.next_logical_id;
            self.next_logical_id += 1;
            id
        };

        core.set_primary(is_primary);
        core.add_thread(logical_id, apic_id);

        dprintf!(
            INFO,
            "APIC: {:#04x} | Logical: {:2} | Package: {:2} | Die: {:2} | Core: {:2} | Thread: {:2} |",
            apic_id,
            logical_id,
            pkg_id,
            die_id,
            core_id,
            smt_id
        );
        if let Some(c_id) = cache_id {
            dprintf!(INFO, " LLC: {:2} |\n", c_id);
        } else {
            dprintf!(INFO, " LLC:  ? |\n");
        }

        Ok(())
    }

    fn into_packages(self) -> fbl::Vector<Option<kalloc::Box<Package>>> {
        self.packages
    }
}

fn generate_tree(
    decoder: &libarch::x86::ApicIdDecoder,
    primary_apic_id: u32,
    cache_info: Option<&libarch::x86::CpuCacheInfo>,
    parser: &dyn acpi_lite::AcpiParserInterface,
) -> Result<fbl::Vector<Option<kalloc::Box<Package>>>, Status> {
    let llc_shift =
        cache_info.and_then(|info| info.as_slice().last()).and_then(|lvl| lvl.share_id_shift);
    let mut pkg_list = PackageList::new(decoder, primary_apic_id, llc_shift);
    acpi_lite::enumerate_processor_local_apics(parser, |entry| pkg_list.add(entry))?;
    Ok(pkg_list.into_packages())
}

fn attach_numa_information(
    parser: &dyn acpi_lite::AcpiParserInterface,
    decoder: &libarch::x86::ApicIdDecoder,
    packages: &mut fbl::Vector<Option<kalloc::Box<Package>>>,
) -> Result<(), Status> {
    acpi_lite::enumerate_cpu_numa_pairs(parser, |domain, apic_id| {
        let pkg_id = decoder.package_id(apic_id) as usize;
        let Some(pkg) = packages.get_mut(pkg_id).and_then(|p| p.as_mut()) else {
            dprintf!(CRITICAL, "ERROR: could not find package #{}\n", pkg_id);
            return;
        };
        let die_id = decoder.die_id(apic_id) as usize;
        let Ok(die) = pkg.get_die(die_id) else {
            dprintf!(
                CRITICAL,
                "ERROR: could not find die #{} within package #{}\n",
                die_id,
                pkg_id
            );
            return;
        };
        if die.numa().is_none() {
            die.set_numa(*domain);
        }
    })
}

fn to_flat_node(numa: &acpi_lite::AcpiNumaDomain) -> zbi::TopologyNode {
    let (start, size) = if numa.memory_count > 0 {
        let mem = numa.memory[0];
        (mem.base_address, mem.length)
    } else {
        (0, 0)
    };
    zbi::TopologyNode {
        entity: zbi::TopologyEntity::NumaRegion(zbi::TopologyNumaRegion { start, size }),
        parent_index: zbi::TOPOLOGY_NO_PARENT,
    }
}

fn flatten_tree(
    packages: &mut fbl::Vector<Option<kalloc::Box<Package>>>,
    flat: &mut fbl::Vector<zbi::TopologyNode>,
) -> Result<(), Status> {
    for pkg_opt in packages.iter_mut() {
        let Some(pkg) = pkg_opt.as_mut() else { continue };

        for die_opt in pkg.dies().iter_mut() {
            let Some(die) = die_opt.as_mut() else { continue };

            if let Some(numa) = die.numa() {
                let numa_flat_index = flat.len() as u16;
                flat.push_back(to_flat_node(numa)).map_err(|_| Status::NO_MEMORY)?;
                die.set_flat_parent(numa_flat_index);
            }

            let die_flat_index = flat.len() as u16;
            flat.push_back(*die.node()).map_err(|_| Status::NO_MEMORY)?;

            for cache_opt in die.caches().iter_mut() {
                let Some(cache) = cache_opt.as_mut() else { continue };

                cache.set_flat_parent(die_flat_index);
                let cache_flat_index = flat.len() as u16;
                flat.push_back(*cache.node()).map_err(|_| Status::NO_MEMORY)?;

                // Add cores that are on a die with shared cache.
                for core_opt in cache.cores().iter_mut() {
                    let Some(core) = core_opt.as_mut() else { continue };
                    core.set_flat_parent(cache_flat_index);
                    flat.push_back(*core.node()).map_err(|_| Status::NO_MEMORY)?;
                }
            }

            // Add cores directly attached to die.
            for core_opt in die.cores().iter_mut() {
                let Some(core) = core_opt.as_mut() else { continue };
                core.set_flat_parent(die_flat_index);
                flat.push_back(*core.node()).map_err(|_| Status::NO_MEMORY)?;
            }
        }
    }
    Ok(())
}

/// Fallback topology node describing a single processor with logical ID 0 and APIC ID 0.
pub const FALLBACK_TOPOLOGY: zbi::TopologyNode = zbi::TopologyNode {
    entity: zbi::TopologyEntity::Processor(zbi::TopologyProcessor {
        architecture_info: zbi::TopologyArchitectureInfo::X64(zbi::TopologyX64Info {
            apic_ids: [0, 0, 0, 0],
            apic_id_count: 1,
        }),
        flags: zbi::TopologyProcessorFlags::PRIMARY,
        logical_ids: [0, 0, 0, 0],
        logical_id_count: 1,
    }),
    parent_index: zbi::TOPOLOGY_NO_PARENT,
};

/// Internal helper for generating flat topology.
fn generate_flat_topology_internal(
    decoder: &libarch::x86::ApicIdDecoder,
    primary_apic_id: u32,
    cache_info: Option<&libarch::x86::CpuCacheInfo>,
    parser: &dyn acpi_lite::AcpiParserInterface,
    topology: &mut fbl::Vector<zbi::TopologyNode>,
) -> Result<(), Status> {
    let mut pkgs = generate_tree(decoder, primary_apic_id, cache_info, parser)?;
    let status = attach_numa_information(parser, decoder, &mut pkgs);
    if status == Err(Status::NOT_FOUND) {
        // This is not a critical error. Systems, such as qemu, may not have the
        // tables present to enumerate NUMA information.
        dprintf!(
            INFO,
            "System topology: Unable to attach NUMA information, missing ACPI tables.\n"
        );
    } else {
        status?;
    }

    flatten_tree(&mut pkgs, topology)
}

/// Generates the flat system topology from CPUID and ACPI data.
/// This is public and exposed for testing.
pub fn generate_flat_topology(
    cpuid: impl regio::x86::Cpuid,
    parser: &dyn acpi_lite::AcpiParserInterface,
    topology: &mut fbl::Vector<zbi::TopologyNode>,
) -> Result<(), Status> {
    let primary_apic_id = libarch::x86::get_apic_id(&cpuid);
    let decoder = libarch::x86::ApicIdDecoder::from_cpuid(&cpuid);
    let cache_info = libarch::x86::CpuCacheInfo::from_cpuid(&cpuid);
    generate_flat_topology_internal(
        &decoder,
        primary_apic_id,
        cache_info.as_ref(),
        parser,
        topology,
    )
}

fn get_global_acpi_lite_parser() -> &'static acpi_lite::AcpiParser<'static> {
    crate::platform_pc::acpi::global_acpi_lite_parser()
}

/// Generates system topology and initializes the system topology graph.
pub fn generate_and_init_system_topology(
    parser: &dyn acpi_lite::AcpiParserInterface,
) -> Result<(), Status> {
    let mut topology = fbl::Vector::new();
    if let Err(status) = generate_flat_topology(regio::x86::DirectCpuid, parser, &mut topology) {
        dprintf!(
            CRITICAL,
            "ERROR: failed to generate flat topology from cpuid and acpi data! : {:?}\n",
            status
        );
        return Err(status);
    }

    crate::topology::Graph::initialize_system_topology(&topology)
}

/// Entry point for initializing system topology at boot.
#[unsafe(no_mangle)]
pub extern "C" fn topology_init() {
    let parser = get_global_acpi_lite_parser();
    if let Err(status) = generate_and_init_system_topology(parser) {
        dprintf!(
            CRITICAL,
            "ERROR: Auto topology generation failed, falling back to only boot core! status: {:?}\n",
            status
        );
        let status = crate::topology::Graph::initialize_system_topology(core::slice::from_ref(
            &FALLBACK_TOPOLOGY,
        ));
        assert_eq!(status, Ok(()));
    }
}

/// x86 system topology tests.
#[cfg(ktest)]
#[unittest::suite(name = "x86_topology_rust")]
mod tests {
    use super::generate_flat_topology;

    struct FakeCpuidRaw<'a> {
        entries: &'a [(u32, u32, u32, u32, u32, u32)],
    }

    impl regio::x86::Cpuid for FakeCpuidRaw<'_> {
        fn read_raw(&self, leaf: u32, subleaf: u32) -> regio::x86::CpuidRawResult {
            for &(l, sl, eax, ebx, ecx, edx) in self.entries {
                if l == leaf && sl == subleaf {
                    return regio::x86::CpuidRawResult { eax, ebx, ecx, edx };
                }
            }
            regio::x86::CpuidRawResult::zeroed()
        }
    }

    struct FakeAcpiParser<'a> {
        tables: &'a [&'a [u8]],
    }

    impl acpi_lite::AcpiParserInterface for FakeAcpiParser<'_> {
        fn num_tables(&self) -> usize {
            self.tables.len()
        }

        fn get_table_at_index(
            &self,
            index: usize,
        ) -> Option<&acpi_lite::structures::AcpiSdtHeader> {
            let table_bytes = self.tables.get(index)?;
            if table_bytes.len() < core::mem::size_of::<acpi_lite::structures::AcpiSdtHeader>() {
                return None;
            }
            let r = zerocopy::Ref::<_, acpi_lite::structures::AcpiSdtHeader>::from_bytes(
                &table_bytes[..core::mem::size_of::<acpi_lite::structures::AcpiSdtHeader>()],
            )
            .ok()?;
            Some(zerocopy::Ref::into_ref(r))
        }
    }

    // CPUID tables from C++ fake-cpuid test dataset.
    const Z840_CPUID_ENTRIES: &[(u32, u32, u32, u32, u32, u32)] = &[
        (0x00000000, 0x00, 0x00000014, 0x756e6547, 0x6c65746e, 0x49656e69),
        (0x00000001, 0x00, 0x000406f1, 0x02200800, 0x7ffefbff, 0xbfebfbff),
        (0x00000002, 0x00, 0x76036301, 0x00f0b5ff, 0x00000000, 0x00c30000),
        (0x00000003, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0x00000004, 0x00, 0x3c004121, 0x01c0003f, 0x0000003f, 0x00000000),
        (0x00000004, 0x01, 0x3c004122, 0x01c0003f, 0x0000003f, 0x00000000),
        (0x00000004, 0x02, 0x3c004143, 0x01c0003f, 0x000001ff, 0x00000000),
        (0x00000004, 0x03, 0x3c07c163, 0x04c0003f, 0x00006fff, 0x00000006),
        (0x00000005, 0x00, 0x00000040, 0x00000040, 0x00000003, 0x00002120),
        (0x00000006, 0x00, 0x00000077, 0x00000002, 0x00000009, 0x00000000),
        (0x00000007, 0x00, 0x00000000, 0x021cbfbb, 0x00000000, 0x9c000400),
        (0x00000008, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0x00000009, 0x00, 0x00000001, 0x00000000, 0x00000000, 0x00000000),
        (0x0000000a, 0x00, 0x07300403, 0x00000000, 0x00000000, 0x00000603),
        (0x0000000b, 0x00, 0x00000001, 0x00000002, 0x00000100, 0x00000002),
        (0x0000000b, 0x01, 0x00000005, 0x0000001c, 0x00000201, 0x00000002),
        (0x0000000c, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0x0000000d, 0x00, 0x00000007, 0x00000340, 0x00000340, 0x00000000),
        (0x0000000d, 0x01, 0x00000001, 0x00000000, 0x00000000, 0x00000000),
        (0x0000000d, 0x02, 0x00000100, 0x00000240, 0x00000000, 0x00000000),
        (0x0000000e, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0x0000000f, 0x00, 0x00000000, 0x0000006f, 0x00000000, 0x00000002),
        (0x0000000f, 0x01, 0x00000000, 0x0000e000, 0x0000006f, 0x00000007),
        (0x00000010, 0x00, 0x00000000, 0x00000002, 0x00000000, 0x00000000),
        (0x00000010, 0x01, 0x00000013, 0x000c0000, 0x00000004, 0x0000000f),
        (0x00000011, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0x00000012, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0x00000013, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0x00000014, 0x00, 0x00000000, 0x00000001, 0x00000001, 0x00000000),
        (0x20000000, 0x00, 0x00000000, 0x00000001, 0x00000001, 0x00000000),
        (0x80000000, 0x00, 0x80000008, 0x00000000, 0x00000000, 0x00000000),
        (0x80000001, 0x00, 0x00000000, 0x00000000, 0x00000121, 0x2c100800),
        (0x80000002, 0x00, 0x65746e49, 0x2952286c, 0x6f655820, 0x2952286e),
        (0x80000003, 0x00, 0x55504320, 0x2d354520, 0x30393632, 0x20347620),
        (0x80000004, 0x00, 0x2e322040, 0x48473036, 0x0000007a, 0x00000000),
        (0x80000005, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0x80000006, 0x00, 0x00000000, 0x00000000, 0x01006040, 0x00000000),
        (0x80000007, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000100),
        (0x80000008, 0x00, 0x0000302e, 0x00000000, 0x00000000, 0x00000000),
        (0x80860000, 0x00, 0x00000000, 0x00000001, 0x00000001, 0x00000000),
        (0xc0000000, 0x00, 0x00000000, 0x00000001, 0x00000001, 0x00000000),
    ];

    const SYS2970WX_CPUID_ENTRIES: &[(u32, u32, u32, u32, u32, u32)] = &[
        (0x00000000, 0x00, 0x0000000d, 0x68747541, 0x444d4163, 0x69746e65),
        (0x00000001, 0x00, 0x00800f82, 0x00300800, 0x7ed8320b, 0x178bfbff),
        (0x00000002, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0x00000003, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0x00000005, 0x00, 0x00000040, 0x00000040, 0x00000003, 0x00000011),
        (0x00000006, 0x00, 0x00000004, 0x00000000, 0x00000001, 0x00000000),
        (0x00000007, 0x00, 0x00000000, 0x209c01a9, 0x00000000, 0x00000000),
        (0x00000008, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0x00000009, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0x0000000a, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0x0000000c, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0x0000000d, 0x00, 0x00000007, 0x00000340, 0x00000340, 0x00000000),
        (0x0000000d, 0x01, 0x0000000f, 0x00000340, 0x00000000, 0x00000000),
        (0x0000000d, 0x02, 0x00000100, 0x00000240, 0x00000000, 0x00000000),
        (0x80000000, 0x00, 0x8000001f, 0x68747541, 0x444d4163, 0x69746e65),
        (0x80000001, 0x00, 0x00800f82, 0x70000000, 0x35c233ff, 0x2fd3fbff),
        (0x80000002, 0x00, 0x20444d41, 0x657a7952, 0x6854206e, 0x64616572),
        (0x80000003, 0x00, 0x70706972, 0x32207265, 0x57303739, 0x34322058),
        (0x80000004, 0x00, 0x726f432d, 0x72502065, 0x7365636f, 0x00726f73),
        (0x80000005, 0x00, 0xff40ff40, 0xff40ff40, 0x20080140, 0x40040140),
        (0x80000006, 0x00, 0x36006400, 0x56006400, 0x02006140, 0x0200c140),
        (0x80000007, 0x00, 0x00000000, 0x0000001b, 0x00000000, 0x00006799),
        (0x80000008, 0x00, 0x00003030, 0x00001007, 0x0000602f, 0x00000000),
        (0x80000009, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0x8000000a, 0x00, 0x00000001, 0x00008000, 0x00000000, 0x0001bcff),
        (0x8000000b, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0x8000000c, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0x8000000d, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0x8000000e, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0x8000000f, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0x80000010, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0x80000011, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0x80000012, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0x80000013, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0x80000014, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0x80000015, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0x80000016, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0x80000017, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0x80000018, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0x80000019, 0x00, 0xf040f040, 0x00000000, 0x00000000, 0x00000000),
        (0x8000001a, 0x00, 0x00000003, 0x00000000, 0x00000000, 0x00000000),
        (0x8000001b, 0x00, 0x000003ff, 0x00000000, 0x00000000, 0x00000000),
        (0x8000001c, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0x8000001d, 0x00, 0x00004121, 0x01c0003f, 0x0000003f, 0x00000000),
        (0x8000001d, 0x01, 0x00004122, 0x00c0003f, 0x000000ff, 0x00000000),
        (0x8000001d, 0x02, 0x00004143, 0x01c0003f, 0x000003ff, 0x00000002),
        (0x8000001d, 0x03, 0x00014163, 0x03c0003f, 0x00001fff, 0x00000001),
        (0x8000001e, 0x00, 0x00000000, 0x00000100, 0x00000300, 0x00000000),
        (0x8000001f, 0x00, 0x0000000f, 0x0000016f, 0x0000000f, 0x00000001),
        (0x80860000, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0xc0000000, 0x00, 0x00000000, 0x00000000, 0x00000000, 0x00000000),
    ];

    /// Enumerate CPUs using data from HP z840.
    #[test]
    fn test_cpus_z840() {
        let cpuid = FakeCpuidRaw { entries: Z840_CPUID_ENTRIES };
        let parser = FakeAcpiParser {
            tables: &[
                acpi_lite::test_data::K_Z840_MADT_TABLE_DATA,
                acpi_lite::test_data::K_Z840_SRAT_TABLE_DATA,
            ],
        };

        let mut flat_topology = fbl::Vector::new();
        let status = generate_flat_topology(cpuid, &parser, &mut flat_topology);
        assert_eq!(status, Ok(()));

        let mut numa_count = 0;
        let mut die_count = 0;
        let mut cache_count = 0;
        let mut core_count = 0;
        let mut thread_count = 0;
        let mut last_numa: i32 = -1;
        let mut last_die: i32 = -1;
        let mut last_cache: i32 = -1;

        for (i, node) in flat_topology.iter().enumerate() {
            match &node.entity {
                zbi::TopologyEntity::NumaRegion(_) => {
                    last_numa = i as i32;
                    numa_count += 1;
                }
                zbi::TopologyEntity::Die(_) => {
                    assert_eq!(last_numa as u16, node.parent_index);
                    last_die = i as i32;
                    die_count += 1;
                }
                zbi::TopologyEntity::Cache(_) => {
                    assert_eq!(last_die as u16, node.parent_index);
                    last_cache = i as i32;
                    cache_count += 1;
                }
                zbi::TopologyEntity::Processor(processor) => {
                    assert_eq!(last_cache as u16, node.parent_index);
                    core_count += 1;
                    thread_count += processor.logical_id_count as usize;
                }
                _ => panic!("Unexpected entity"),
            }
        }

        // We expect two numa regions, two dies, two caches, and 28 cores.
        assert_eq!(2 + 2 + 2 + 28, flat_topology.len());
        assert_eq!(2, numa_count);
        assert_eq!(2, die_count);
        assert_eq!(2, cache_count);
        assert_eq!(28, core_count);
        assert_eq!(56, thread_count);

        // Ensure the format can be parsed and validated by the system topology library.
        let mut graph = crate::topology::Graph::new();
        assert_eq!(crate::topology::Graph::initialize(&mut graph, &flat_topology), Ok(()));
    }

    /// Enumerate CPUs using data from ThreadRipper 2970wx/X399.
    #[test]
    fn test_cpus_2970wx_x399() {
        let cpuid = FakeCpuidRaw { entries: SYS2970WX_CPUID_ENTRIES };
        let parser = FakeAcpiParser {
            tables: &[
                acpi_lite::test_data::K2970WX_MADT_TABLE_DATA,
                acpi_lite::test_data::K2970WX_SRAT_TABLE_DATA,
            ],
        };

        let mut flat_topology = fbl::Vector::new();
        let status = generate_flat_topology(cpuid, &parser, &mut flat_topology);
        assert_eq!(status, Ok(()));

        let mut numa_count = 0;
        let mut die_count = 0;
        let mut core_count = 0;
        let mut cache_count = 0;
        let mut thread_count = 0;
        let mut last_numa: i32 = -1;
        let mut last_die: i32 = -1;
        let mut last_cache: i32 = -1;

        for (i, node) in flat_topology.iter().enumerate() {
            match &node.entity {
                zbi::TopologyEntity::NumaRegion(_) => {
                    last_numa = i as i32;
                    numa_count += 1;
                }
                zbi::TopologyEntity::Die(_) => {
                    assert_eq!(last_numa as u16, node.parent_index);
                    last_die = i as i32;
                    die_count += 1;
                }
                zbi::TopologyEntity::Cache(_) => {
                    assert_eq!(last_die as u16, node.parent_index);
                    last_cache = i as i32;
                    cache_count += 1;
                }
                zbi::TopologyEntity::Processor(processor) => {
                    assert_eq!(last_cache as u16, node.parent_index);
                    core_count += 1;
                    thread_count += processor.logical_id_count as usize;
                }
                _ => panic!("Unexpected entity"),
            }
        }

        assert_eq!(4, numa_count);
        assert_eq!(4, die_count);
        assert_eq!(8, cache_count);
        assert_eq!(24, core_count);
        assert_eq!(48, thread_count);

        // Ensure the format can be parsed and validated by the system topology library.
        let mut graph = crate::topology::Graph::new();
        assert_eq!(crate::topology::Graph::initialize(&mut graph, &flat_topology), Ok(()));
    }

    /// Enumerate CPUs using data triggering fallback.
    #[test]
    fn test_cpus_fallback() {
        // With an 'empty' CPUID data set (representing a pathological case) we would
        // expect enumeration to fall back to maximally flat description of one
        // thread to one core to one package, multiplied by the number of processors
        // enumerated by ACPI.
        let cpuid = FakeCpuidRaw { entries: &[] };
        let parser = FakeAcpiParser {
            tables: &[
                acpi_lite::test_data::K_EVE_MADT_TABLE_DATA,
                acpi_lite::test_data::K_EVE_HPET_TABLE_DATA,
            ],
        };

        let mut flat_topology = fbl::Vector::new();
        let status = generate_flat_topology(cpuid, &parser, &mut flat_topology);
        assert_eq!(status, Ok(()));

        let mut numa_count = 0;
        let mut die_count = 0;
        let mut core_count = 0;
        let mut cache_count = 0;
        let mut thread_count = 0;

        for node in flat_topology.iter() {
            match &node.entity {
                zbi::TopologyEntity::NumaRegion(_) => {
                    numa_count += 1;
                }
                zbi::TopologyEntity::Die(_) => {
                    die_count += 1;
                }
                zbi::TopologyEntity::Cache(_) => {
                    cache_count += 1;
                }
                zbi::TopologyEntity::Processor(processor) => {
                    core_count += 1;
                    thread_count += processor.logical_id_count as usize;
                }
                _ => panic!("Unexpected entity"),
            }
        }

        assert_eq!(0, numa_count);
        assert_eq!(4, die_count);
        assert_eq!(0, cache_count);
        assert_eq!(4, core_count);
        assert_eq!(4, thread_count);

        // Ensure the format can be parsed and validated by the system topology library.
        let mut graph = crate::topology::Graph::new();
        assert_eq!(crate::topology::Graph::initialize(&mut graph, &flat_topology), Ok(()));
    }
}
