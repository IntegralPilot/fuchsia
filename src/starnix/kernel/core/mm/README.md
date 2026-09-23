# Starnix Memory Manager

The memory manager provides Linux memory manipulation operations and maintains the
Linux address space in conjunction with Zircon.


## Address space

The address space exposed to Linux programs is contained within one Zircon
virtual memory address range (VMAR) object. This object covers all of the
userspace exposed range. Starnix makes all decisions about where allocations are
placed inside the VMAR and Starnix alone is responsible for applying an ASLR
policy on those allocations. Starnix additionally maintains a data structure
containing information about all Linux-exposed allocations and the memory ranges
they cover.

## Linux and Zircon models

The Zircon memory model is based on objects and the Linux model is based on
address ranges. In Zircon, memory is allocated by creating and resizing VMOs
and memory mappings are changed through operations on VMARs. In Linux,
operations on address ranges such as `mmap()`/`munmap()`/`mremap()` both
allocate/deallocate memory and modify memory mappings. The memory manager
bridges between these models by maintaining a map of Linux address ranges to
backing Zircon objects. Linux memory operations like `mmap()` update both
Starnix's model and Zircon's address space mapping. Memory allocations from
Linux can be backed by VMOs created by the memory manager, by VMOs created by
other places inside Starnix and provided to the memory manager, or by VMOs
supplied by an external service such as a Fuchsia filesystem server.


## Private anonymous memory

To efficiently support private anonymous memory allocations (the most common
type of allocation), Starnix uses a single large VMO to back all such mappings
within an address space, rather than creating a new VMO for each allocation.

This "Big VMO" is sized to cover the entire user address space. The offset
into the VMO for any given page corresponds directly to its virtual address in
the Linux address space.

This design offers several advantages:
- **Handle count reduction**: It avoids the explosion of Zircon handles and VMO
  objects that would occur if every small allocation required its own VMO.
- **Efficient deallocation**: When a range is unmapped, Starnix can immediately
  deallocate the corresponding pages in the big VMO, knowing they cannot be
  referenced by other mappings.
- **Simplified offset calculation**: The offset in the VMO is simply the user
  virtual address.

This behavior is visible in the implementation of `get_mapping_memory` in
`memory_manager.rs`, where the offset returned for a `PrivateAnonymous` mapping
is simply the pointer value of the address.


## Unmap and remap

Linux supports unmapping and remapping ranges of memory that may overlap with
existing mappings. To support this, the memory manager must translate these
range operations into operations on specific objects. In some cases this means
creating additional objects to represent a mapping of parts of an allocation.
Consider the scenario where a Linux program creates an anonymous shared
mapping 3 pages long. The memory manager will allocate a VMO to back this
memory and associate it with the mapped range:


```
0x1234...0000, 0x1234...3000 -> Mapped memory backed by VMO A
```

Then the Linux program unmaps the range 0x1234...1000 -> 0x1234...2000. To
handle this, the memory manager first creates a snapshot child covering the
last page of VMO A to use for the top part of the mapping and then resizes
VMO A down to cover the bottom part of the mapping:


```
0x1234...0000, 0x1234...1000 -> Mapped memory backed by VMO A (resized)
0x1234...1000, 0x1234...2000 -> Unmapped
0x1234...2000, 0x1234...3000 -> Mapped memory backed by child VMO
```

## Lazy Mapping

To optimize memory usage and reduce overhead during task creation, Starnix
supports lazy mapping. When a lazy mapping is created, the memory manager
defers the actual Zircon VMAR allocation and mapping until the memory is
actively accessed.

Mappings track their state via a `MappingMode` (either `Eager` or `Lazy`),
which is reflected in the `MAPPED_IN_VMAR` flag in `MappingFlags`. When access
occurs, the memory manager materializes the mapping by mapping the backing VMO
into the VMAR.

Materialization is triggered in several ways:
- **User mode access**: Page faults on unmapped ranges that correspond to lazy
  mappings are handled by extending or materializing the mapping.
- **Usercopy routines**: Accesses from kernel mode via usercopy routines (e.g.,
  in `unified_read_memory` and `unified_write_memory`) handle faults
  reactively. If a copy fails to transfer any bytes, the transfer loop
  materializes the mapping for that page and retries the operation.
- **C FFI Calls**: In cases where memory is passed to external C FFI calls
  (e.g., `zxio.sendmsg` or `recvmsg`), the memory regions must be
  pre-materialized because the kernel cannot catch faults directly as it does
  for Starnix-internal usercopies.

## User and kernel mode access

### User mode

Linux programs running in user mode access memory directly through the memory
mappings established by Zircon. Access faults are forwarded from Zircon to the
Starnix executor. Some faults, such as page-not-present page faults, are
forwarded to the memory manager to handle growth of `MAP_GROWSDOWN` segments
(generally used for stack memory) or to materialize lazy mappings.

### Kernel mode

The most common case for access from Starnix's "kernel mode" to user memory is
to use usercopy routines that handle materialization transparently. We use that
whenever accessing memory of an address space that is currently mapped in (i.e.
from the same thread).

Alternatively, access can be handled by first examining the memory manager map
to identify the VMO(s) backing the range of interest. If the mapping is lazy,
the memory manager materializes it before proceeding. Once materialized, it
issues `zx_vmo_read`/`zx_vmo_write` calls to interact with these objects.

## Invariants

The memory manager must make sure that it is in a consistent state from
internal (kernel mode) and external (user mode) POVs. That is, write operations
on memory manager (e.g. mmap, munmap, remap, etc.) must never allow another
operation to consider partially updated state as final. This is because there
exists certain read operations (e.g. vmsplice, fork, clone) that take a snapshot
of the whole memory manager, or a subset of its mappings, and that snapshot
must represent a valid state of memory for user space.
