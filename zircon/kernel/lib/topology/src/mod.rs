// Copyright 2018 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Captures the physical layout of the core system (processors, caches, etc..).
//! The data will be laid out as a tree, with processor nodes on the bottom and other types above
//! them. The expected usage is to start from a processor node and walk up/down to discover the
//! relationships you are interested in.

use crate::kernel::types::cpu_num_t;
use core::ptr::NonNull;
use debug::{dprintf, ltracef};
use zerocopy::TryFromBytes;
use zx_status::Status;

const LOCAL_TRACE: u32 = 0;

const MAX_TOPOLOGY_DEPTH: usize = 20;

#[inline]
fn validation_error(index: usize, message: &str) {
    kprint::kprintln!("Error validating topology at node {:u} : {:s}", index, message);
}

fn grow_vector<T: Default>(new_size: usize, vector: &mut fbl::Vector<T>) -> Result<(), Status> {
    for _ in vector.len()..new_size {
        vector.push_back(T::default()).map_err(|_| Status::NO_MEMORY)?;
    }
    Ok(())
}

fn entity_discriminant(entity: &zbi::TopologyEntity) -> u64 {
    match entity {
        zbi::TopologyEntity::Processor(_) => 1,
        zbi::TopologyEntity::Cluster(_) => 2,
        zbi::TopologyEntity::Cache(_) => 3,
        zbi::TopologyEntity::Die(_) => 4,
        zbi::TopologyEntity::Socket(_) => 5,
        zbi::TopologyEntity::NumaRegion(_) => 6,
    }
}

fn zbi_topology_type_to_string(entity: &zbi::TopologyEntity) -> &'static str {
    match entity {
        zbi::TopologyEntity::Processor(_) => "processor",
        zbi::TopologyEntity::Cluster(_) => "cluster",
        zbi::TopologyEntity::Cache(_) => "cache",
        zbi::TopologyEntity::Die(_) => "die",
        zbi::TopologyEntity::Socket(_) => "socket",
        zbi::TopologyEntity::NumaRegion(_) => "numa_region",
    }
}

/// FFI-compatible slice representation matching C++ `ktl::span<T>`.
#[repr(C)]
struct FfiSlice<T> {
    ptr: Option<NonNull<T>>,
    count: usize,
}

impl<T> Clone for FfiSlice<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for FfiSlice<T> {}

impl<T> Default for FfiSlice<T> {
    fn default() -> Self {
        Self::empty()
    }
}

impl<T> FfiSlice<T> {
    /// Creates an empty `FfiSlice`.
    const fn empty() -> Self {
        Self { ptr: None, count: 0 }
    }

    /// Creates an `FfiSlice` pointing to the elements of `slice`.
    fn from_slice(slice: &[T]) -> Self {
        if slice.is_empty() {
            Self::empty()
        } else {
            Self { ptr: NonNull::new(slice.as_ptr() as *mut T), count: slice.len() }
        }
    }

    /// Converts this `FfiSlice` into a Rust slice reference.
    ///
    /// # Safety
    ///
    /// If `self.count > 0`, `self.ptr` must be `Some(ptr)`, properly aligned, and point to
    /// `self.count` initialized elements of type `T` that remain valid for the lifetime `'a`.
    unsafe fn as_slice<'a>(&self) -> &'a [T] {
        match self.ptr {
            Some(ptr) if self.count > 0 => {
                // SAFETY: The caller guarantees `ptr` points to `self.count` valid elements.
                unsafe { core::slice::from_raw_parts(ptr.as_ptr(), self.count) }
            }
            _ => &[],
        }
    }
}

impl FfiSlice<NonNull<Node>> {
    /// Converts an `FfiSlice<NonNull<Node>>` into a slice of `&'a Node` references.
    ///
    /// # Safety
    ///
    /// If `self.count > 0`, `self.ptr` must be `Some(ptr)`, properly aligned, and point to
    /// `self.count` valid `NonNull<Node>` pointers to initialized `Node`s that remain valid
    /// for the lifetime `'a`.
    unsafe fn as_node_slice<'a>(&self) -> &'a [&'a Node] {
        match self.ptr {
            Some(ptr) if self.count > 0 => {
                // SAFETY: `NonNull<Node>` and `&'a Node` have identical memory layout and alignment.
                // The caller guarantees each pointer in the slice points to a valid `Node` for `'a`.
                unsafe { core::slice::from_raw_parts(ptr.as_ptr() as *const &'a Node, self.count) }
            }
            _ => &[],
        }
    }
}

/// A single node in the topology graph.
#[repr(C)]
pub struct Node {
    entity: zbi::TopologyEntity,
    parent: Option<NonNull<Node>>,
    children: FfiSlice<NonNull<Node>>,
}

// SAFETY: `Node` contains pointers (`parent` and `children`) to other `Node`s within the
// same `Graph` allocation. Once constructed, nodes in a `Graph` are read-only.
unsafe impl Send for Node {}
// SAFETY: As above; shared access only ever reads.
unsafe impl Sync for Node {}

impl Default for Node {
    fn default() -> Self {
        Self {
            entity: zbi::TopologyEntity::Die(zbi::TopologyDie { reserved: 0 }),
            parent: None,
            children: FfiSlice::empty(),
        }
    }
}

impl Node {
    /// Returns a reference to the ZBI topology entity associated with this node.
    pub fn entity(&self) -> &zbi::TopologyEntity {
        &self.entity
    }

    /// Returns a reference to the parent node, or `None` if this node has no parent.
    pub fn parent(&self) -> Option<&Node> {
        // SAFETY: If `self.parent` is `Some(ptr)`, it points to a valid `Node` in the owning `Graph`.
        self.parent.map(|ptr| unsafe { ptr.as_ref() })
    }

    /// Returns a slice of references to this node's children.
    pub fn children(&self) -> &[&Node] {
        // SAFETY: `self.children` points to valid child node pointers owned by `GraphBacking`.
        unsafe { self.children.as_node_slice() }
    }
}

/// Internal heap backing storage owning the containers referenced by a `Graph`.
struct GraphBacking {
    _nodes: fbl::Array<Node>,
    _processors: fbl::Vector<NonNull<Node>>,
    _processors_by_logical_id: fbl::Vector<Option<NonNull<Node>>>,
    _children_vectors: fbl::Array<fbl::Vector<NonNull<Node>>>,
}

/// A view of the system topology that is defined in early boot and static during the run of the
/// system.
#[repr(C)]
pub struct Graph {
    nodes: FfiSlice<Node>,
    processors: FfiSlice<NonNull<Node>>,
    logical_processor_count: usize,
    // This is in essence a map with logical ID being the index in the vector.
    // It will contain duplicates for SMT processors so we need it in addition to processors.
    processors_by_logical_id: FfiSlice<Option<NonNull<Node>>>,
    backing: Option<NonNull<GraphBacking>>,
}

// SAFETY: `Graph` owns its heap backing storage exclusively and is read-only once initialized.
unsafe impl Send for Graph {}
// SAFETY: As above; shared access only ever reads.
unsafe impl Sync for Graph {}

zr::static_assert!(core::mem::size_of::<FfiSlice<NonNull<Node>>>() == 16);
zr::static_assert!(core::mem::align_of::<FfiSlice<NonNull<Node>>>() == 8);
zr::static_assert!(core::mem::size_of::<NonNull<Node>>() == core::mem::size_of::<&Node>());
zr::static_assert!(core::mem::align_of::<NonNull<Node>>() == core::mem::align_of::<&Node>());
zr::static_assert!(core::mem::size_of::<Node>() == 80);
zr::static_assert!(core::mem::align_of::<Node>() == 8);
zr::static_assert!(core::mem::offset_of!(Node, entity) == 0);
zr::static_assert!(core::mem::offset_of!(Node, parent) == 56);
zr::static_assert!(core::mem::offset_of!(Node, children) == 64);
zr::static_assert!(core::mem::size_of::<Graph>() == 64);
zr::static_assert!(core::mem::align_of::<Graph>() == 8);
zr::static_assert!(core::mem::offset_of!(Graph, nodes) == 0);
zr::static_assert!(core::mem::offset_of!(Graph, processors) == 16);
zr::static_assert!(core::mem::offset_of!(Graph, logical_processor_count) == 32);
zr::static_assert!(core::mem::offset_of!(Graph, processors_by_logical_id) == 40);
zr::static_assert!(core::mem::offset_of!(Graph, backing) == 56);
zr::static_assert!(core::mem::size_of::<Result<(), Status>>() == 4);
zr::static_assert!(core::mem::align_of::<Result<(), Status>>() == 4);

impl Default for Graph {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Graph {
    fn drop(&mut self) {
        if let Some(backing) = self.backing.take() {
            // SAFETY: `backing` was allocated via `kalloc::Box::into_raw` in
            // `Graph::initialize` and is uniquely owned by this `Graph` instance.
            unsafe {
                let _ = kalloc::Box::from_raw(backing.as_ptr());
            }
            self.nodes = FfiSlice::empty();
            self.processors = FfiSlice::empty();
            self.logical_processor_count = 0;
            self.processors_by_logical_id = FfiSlice::empty();
        }
    }
}

/// The graph of the system topology. Initialized once during early boot.
static SYSTEM_TOPOLOGY: lazy_init::LazyInit<Graph> = lazy_init::LazyInit::uninit();

impl Graph {
    /// Graph instances are default constructible to empty.
    pub const fn new() -> Self {
        Self {
            nodes: FfiSlice::empty(),
            processors: FfiSlice::empty(),
            logical_processor_count: 0,
            processors_by_logical_id: FfiSlice::empty(),
            backing: None,
        }
    }

    /// Initializes the system topology `Graph` instance from the given flat
    /// topology. Performs validation on the flat topology before updating the
    /// system graph with the unflattened data. If validation fails an error is
    /// returned the system graph is left unmodified in its original state.
    ///
    /// Note that there is no explicit synchronization protecting concurrent
    /// access to the system topology. It is expected to be initialized once at
    /// early boot and then remain static and read-only. Relaxing this constraint
    /// is possible by adding internal synchronization.
    ///
    /// Returns `Err(Status::NO_MEMORY)` if dynamic memory allocation fails.
    /// Returns `Err(Status::INVALID_ARGS)` if validation of the flat topology fails.
    pub fn initialize_system_topology(nodes: &[zbi::TopologyNode]) -> Result<(), Status> {
        if nodes.is_empty() {
            return Err(Status::INVALID_ARGS);
        }

        let mut graph = Graph::new();
        Self::initialize(&mut graph, nodes)?;

        // Initialize the global system topology graph instance.
        // SAFETY: `initialize_system_topology` is called once during early boot before
        // concurrent readers access `SYSTEM_TOPOLOGY`.
        unsafe {
            SYSTEM_TOPOLOGY.init(graph);
        }
        Ok(())
    }

    /// Initializes the given topology `Graph` instance from the given flat
    /// topology. Performs validation on the flat topology before updating the
    /// `graph` with the unflattened data. If validation fails an error is
    /// returned and `graph` is left unmodified in its original state.
    ///
    /// Returns `Err(Status::NO_MEMORY)` if dynamic memory allocation fails.
    /// Returns `Err(Status::INVALID_ARGS)` if validation of the flat topology fails.
    pub fn initialize(graph: &mut Graph, flat_nodes: &[zbi::TopologyNode]) -> Result<(), Status> {
        debug_assert!(!flat_nodes.is_empty());

        let count = flat_nodes.len();
        ltracef!("count {}\n", count);

        if count == 0 || !Self::validate(flat_nodes) {
            return Err(Status::INVALID_ARGS);
        }

        let mut nodes = fbl::Array::<Node>::try_new(count).map_err(|_| Status::NO_MEMORY)?;
        let mut children_vectors = fbl::Array::<fbl::Vector<NonNull<Node>>>::try_new(count)
            .map_err(|_| Status::NO_MEMORY)?;

        // Create local instances, if successful we will move them to the Graph's fields.
        let mut processors = fbl::Vector::<NonNull<Node>>::new();
        let mut processors_by_logical_id = fbl::Vector::<Option<NonNull<Node>>>::new();
        let mut logical_processor_count: usize = 0;

        let nodes_ptr = nodes.as_mut_ptr();

        for (flat_node_index, flat_node) in flat_nodes.iter().enumerate() {
            // SAFETY: `flat_node_index < count`, so `nodes_ptr.add(flat_node_index)` is
            // in-bounds of the heap-allocated `nodes` array. All accesses to `nodes` go through
            // `nodes_ptr` to preserve Stacked Borrows pointer provenance.
            let raw_node_ptr = unsafe { nodes_ptr.add(flat_node_index) };
            let node_nn = NonNull::new(raw_node_ptr).unwrap();
            // SAFETY: `raw_node_ptr` is in-bounds as above, and no other reference to this
            // element is live; other nodes hold only raw pointers to it.
            let node = unsafe { &mut *raw_node_ptr };

            // Copies the type along with its attached info, for every entity type.
            node.entity = flat_node.entity;
            ltracef!(
                "index {} type {} ({})\n",
                flat_node_index,
                entity_discriminant(&node.entity),
                zbi_topology_type_to_string(&node.entity)
            );

            // Processors are additionally indexed by position and by logical id.
            if let zbi::TopologyEntity::Processor(processor) = &node.entity {
                processors.push_back(node_nn).map_err(|_| Status::NO_MEMORY)?;
                logical_processor_count += processor.logical_id_count as usize;

                for &logical_id in &processor.logical_ids[..processor.logical_id_count as usize] {
                    let index = logical_id as usize;
                    grow_vector(index + 1, &mut processors_by_logical_id)?;
                    processors_by_logical_id[index] = Some(node_nn);
                }
            }

            if flat_node.parent_index != zbi::TOPOLOGY_NO_PARENT {
                // Validation should have prevented this.
                let parent_idx = flat_node.parent_index as usize;
                debug_assert!(
                    parent_idx < count,
                    "parent_index out of range: {}\n",
                    flat_node.parent_index
                );

                // SAFETY: `parent_idx < count`, so `nodes_ptr.add(parent_idx)` is
                // in-bounds of `nodes`.
                let parent_nn = NonNull::new(unsafe { nodes_ptr.add(parent_idx) }).unwrap();
                node.parent = Some(parent_nn);
                children_vectors[parent_idx].push_back(node_nn).map_err(|_| Status::NO_MEMORY)?;
            }
        }

        for i in 0..count {
            // SAFETY: `i < count`, accessing exclusively through `nodes_ptr` to preserve provenance.
            let node = unsafe { &mut *nodes_ptr.add(i) };
            node.children = FfiSlice::from_slice(&children_vectors[i]);
        }

        let nodes_slice = FfiSlice { ptr: NonNull::new(nodes_ptr), count };
        let processors_slice = FfiSlice::from_slice(&processors);
        let processors_by_logical_id_slice = FfiSlice::from_slice(&processors_by_logical_id);

        let backing = kalloc::Box::try_new(GraphBacking {
            _nodes: nodes,
            _processors: processors,
            _processors_by_logical_id: processors_by_logical_id,
            _children_vectors: children_vectors,
        })
        .map_err(|_| Status::NO_MEMORY)?;

        *graph = Graph {
            nodes: nodes_slice,
            processors: processors_slice,
            logical_processor_count,
            processors_by_logical_id: processors_by_logical_id_slice,
            backing: NonNull::new(kalloc::Box::into_raw(backing)),
        };

        graph.dump();

        Ok(())
    }

    /// Provides iterable slice of references to all processor nodes.
    pub fn processors(&self) -> &[&Node] {
        // SAFETY: `self.processors` points to valid `NonNull<Node>` elements owned by `GraphBacking`.
        unsafe { self.processors.as_node_slice() }
    }

    /// Number of processor nodes in the topology, this is equivalent to the
    /// number of physical processor cores.
    pub fn processor_count(&self) -> usize {
        self.processors.count
    }

    /// Number of logical processors in system, this will be different from
    /// `processor_count()` if the system supports SMT.
    pub fn logical_processor_count(&self) -> usize {
        self.logical_processor_count
    }

    /// Finds the processor node that is assigned the given logical id.
    /// Returns a reference to that node, or `Err(Status::NOT_FOUND)` if it wasn't found.
    pub fn processor_by_logical_id(&self, id: cpu_num_t) -> Result<&Node, Status> {
        // SAFETY: `self.processors_by_logical_id` points to valid elements owned by `GraphBacking`.
        let slice = unsafe { self.processors_by_logical_id.as_slice() };
        let Some(Some(ptr)) = slice.get(id as usize) else {
            return Err(Status::NOT_FOUND);
        };
        // SAFETY: Non-null pointers in `processors_by_logical_id` point to valid
        // `Node` instances in `self.nodes` owned by `GraphBacking`.
        Ok(unsafe { ptr.as_ref() })
    }

    /// Returns an immutable reference to the system topology graph. This may be
    /// called after the graph is initialized by `Graph::initialize_system_topology`.
    pub fn get_system_topology() -> &'static Graph {
        SYSTEM_TOPOLOGY.get()
    }

    /// Not a fantastic dump routine, but displays everything in the tree, leaf -> root
    /// for every leaf node.
    pub fn dump(&self) {
        kprint::kprintln!("Topology graph (leaves to root):");

        // synthesize a unique id per node in the graph based on the pointer in
        // the array of nodes.
        let nodes_base = self.nodes.ptr.map_or(0, |p| p.as_ptr() as usize);
        let node_to_id = |n: &Node| -> usize {
            (n as *const Node as usize).wrapping_sub(nodes_base) / core::mem::size_of::<Node>()
        };

        for &cpu in self.processors() {
            let logical_id_0 = match &cpu.entity {
                zbi::TopologyEntity::Processor(proc) => proc.logical_ids[0],
                _ => {
                    debug_assert!(false, "Expected processor entity in processors slice");
                    0
                }
            };
            kprint::kprint!("processor {:u}", logical_id_0);
            let mut cur = cpu.parent();
            while let Some(node) = cur {
                kprint::kprint!(
                    " -> {:s} (id {:u}) ",
                    zbi_topology_type_to_string(&node.entity),
                    node_to_id(node)
                );
                cur = node.parent();
            }
            kprint::kprintln!("");
        }
    }

    /// Validates that in the provided flat topology:
    ///   - all processors are leaf nodes, and all leaf nodes are processors.
    ///   - there are no cycles.
    ///   - It is stored in a "depth first" ordering, with parents adjacent to
    ///     their children.
    fn validate(nodes: &[zbi::TopologyNode]) -> bool {
        debug_assert!(!nodes.is_empty());

        let mut parents = [zbi::TOPOLOGY_NO_PARENT; MAX_TOPOLOGY_DEPTH];
        let mut current_type: Option<u64> = None;
        let mut current_depth: usize = 0;

        let count = nodes.len();
        for index in 0..count {
            // Traverse the nodes in reverse order.
            let current_index = count - index - 1;
            let node = &nodes[current_index];

            if node.parent_index != zbi::TOPOLOGY_NO_PARENT && (node.parent_index as usize) >= count
            {
                validation_error(current_index, "Parent index out of bounds.");
                return false;
            }

            if let zbi::TopologyEntity::Processor(processor) = &node.entity
                && processor.logical_id_count as usize > processor.logical_ids.len()
            {
                validation_error(current_index, "Processor logical_id_count exceeds capacity.");
                return false;
            }

            let discriminant = entity_discriminant(&node.entity);
            let is_processor = matches!(node.entity, zbi::TopologyEntity::Processor(_));

            if let Some(curr_type) = current_type {
                if curr_type != discriminant {
                    if parents[current_depth] != zbi::TOPOLOGY_NO_PARENT
                        && current_index == parents[current_depth] as usize
                    {
                        // If the type changes then it should be the parent of the
                        // previous level.
                        current_depth += 1;

                        if current_depth == MAX_TOPOLOGY_DEPTH {
                            validation_error(
                                current_index,
                                "Structure is too deep, we only support 20 levels.",
                            );
                            return false;
                        }
                    } else if is_processor {
                        // If it isn't the parent of the previous level, but it is a processor then we have
                        // encountered a new branch and should start walking from the bottom again.

                        // Clear the parent indices for all levels but the top, we want to ensure that the
                        // top level of the new branch reports to the same parent as we do.
                        for i in (0..current_depth).rev() {
                            parents[i] = zbi::TOPOLOGY_NO_PARENT;
                        }
                        current_depth = 0;
                    } else {
                        // Otherwise the structure is incorrect.
                        validation_error(
                            current_index,
                            "Graph is not stored in correct order, with children adjacent to parents",
                        );
                        return false;
                    }
                    current_type = Some(discriminant);
                }
            } else {
                current_type = Some(discriminant);
            }

            if parents[current_depth] == zbi::TOPOLOGY_NO_PARENT {
                parents[current_depth] = node.parent_index;
            } else if parents[current_depth] != node.parent_index {
                validation_error(current_index, "Parents at level do not match.");
                return false;
            }

            // Ensure that all leaf nodes are processors.
            if current_depth == 0 && !is_processor {
                validation_error(current_index, "Encountered a leaf node that isn't a processor.");
                return false;
            }

            // Ensure that all processors are leaf nodes.
            if current_depth != 0 && is_processor {
                validation_error(current_index, "Encountered a processor that isn't a leaf node.");
                return false;
            }

            // By the time we reach the first parent we should be at the maximum depth and have no
            // parents defined.
            if current_index == 0
                && parents[current_depth] != zbi::TOPOLOGY_NO_PARENT
                && (current_depth == MAX_TOPOLOGY_DEPTH - 1
                    || parents[current_depth + 1] == zbi::TOPOLOGY_NO_PARENT)
            {
                validation_error(current_index, "Top level of tree should not have a parent");
                return false;
            }
        }
        true
    }
}

/// Returns an immutable reference to the system topology graph. This may be
/// called after the graph is initialized by `Graph::initialize_system_topology`.
#[inline]
pub fn get_system_topology() -> &'static Graph {
    Graph::get_system_topology()
}

/// Looks up the performance class of the given logical CPU. Returns zero if no
/// cpu or cluster node is found.
pub fn get_performance_class(cpu_id: cpu_num_t) -> u8 {
    let Ok(cpu_node) = get_system_topology().processor_by_logical_id(cpu_id) else {
        dprintf!(INFO, "System topology: Failed to get processor node for cpu {}\n", cpu_id);
        return 0;
    };

    let mut current = cpu_node.parent();
    while let Some(node) = current {
        if let zbi::TopologyEntity::Cluster(ref cluster) = *node.entity() {
            return cluster.performance_class;
        }
        current = node.parent();
    }

    0
}

/// Views `count` C `zbi_topology_node_t`s as a `zbi::TopologyNode` slice.
///
/// The memory typically comes from the bootloader, so the encoding of each node is checked:
/// `zbi::TopologyEntity` and `zbi::TopologyArchitectureInfo` are Rust enums, and a reference to
/// one with an unknown discriminant is undefined behavior. Returns `Err(Status::INVALID_ARGS)`
/// if any node is not a valid `zbi::TopologyNode`.
///
/// # Safety
///
/// `nodes` must be non-null and point to `count * size_of::<zbi::TopologyNode>()` readable bytes
/// that remain valid and unmodified for `'a`.
unsafe fn flat_nodes_from_raw<'a>(
    nodes: *const zbi::TopologyNode,
    count: usize,
) -> Result<&'a [zbi::TopologyNode], Status> {
    let size =
        count.checked_mul(core::mem::size_of::<zbi::TopologyNode>()).ok_or(Status::INVALID_ARGS)?;
    // SAFETY: The caller guarantees `nodes` points to `size` readable bytes.
    let bytes = unsafe { core::slice::from_raw_parts(nodes.cast::<u8>(), size) };
    <[zbi::TopologyNode]>::try_ref_from_bytes(bytes).map_err(|_| {
        kprint::kprintln!("Error validating topology : Invalid node encoding.");
        Status::INVALID_ARGS
    })
}

/// Initializes the global system topology graph from a flat array of `zbi_topology_node_t`s.
///
/// # Safety
///
/// If `count > 0`, `nodes` must be non-null and point to `count` readable
/// `zbi_topology_node_t`-sized elements. Their contents need not be valid.
#[allow(improper_ctypes_definitions)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_system_topology_initialize_system_topology(
    nodes: *const zbi::TopologyNode,
    count: usize,
) -> Result<(), Status> {
    if count == 0 || nodes.is_null() {
        return Err(Status::INVALID_ARGS);
    }
    // SAFETY: `nodes` is non-null and the caller guarantees it points to `count` elements.
    let slice = unsafe { flat_nodes_from_raw(nodes, count) }?;
    Graph::initialize_system_topology(slice)
}

/// Initializes the given `Graph` instance from a flat array of `zbi_topology_node_t`s.
///
/// # Safety
///
/// If `count > 0`, `nodes` must be non-null and point to `count` readable
/// `zbi_topology_node_t`-sized elements. Their contents need not be valid.
#[allow(improper_ctypes_definitions)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_system_topology_graph_initialize(
    graph: &mut Graph,
    nodes: *const zbi::TopologyNode,
    count: usize,
) -> Result<(), Status> {
    debug_assert!(!nodes.is_null());
    debug_assert!(count > 0);
    if count == 0 || nodes.is_null() {
        return Err(Status::INVALID_ARGS);
    }
    // SAFETY: `nodes` is non-null and the caller guarantees it points to `count` elements.
    let slice = unsafe { flat_nodes_from_raw(nodes, count) }?;
    Graph::initialize(graph, slice)
}

/// Destroys the given `Graph` instance and frees its heap-allocated backing storage.
#[unsafe(no_mangle)]
pub extern "C" fn rust_system_topology_graph_destroy(graph: &mut Graph) {
    *graph = Graph::new();
}

/// Returns a reference to the global system topology `Graph`.
#[unsafe(no_mangle)]
pub extern "C" fn rust_system_topology_get_system_topology() -> &'static Graph {
    Graph::get_system_topology()
}

/// Dumps the contents of the given `Graph` to kernel output.
#[unsafe(no_mangle)]
pub extern "C" fn rust_system_topology_graph_dump(graph: &Graph) {
    graph.dump();
}

/// Looks up the performance class of the given logical CPU ID.
#[unsafe(no_mangle)]
pub extern "C" fn rust_system_topology_get_performance_class(cpu_id: cpu_num_t) -> u8 {
    get_performance_class(cpu_id)
}

#[cfg(ktest)]
mod tests;
