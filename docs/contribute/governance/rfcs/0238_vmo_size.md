<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0238" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Summary

Previously, Zircon associated two different sizes with each VMO: the *size* of
the VMO, at page granularity, and the *content size* of the VMO, at byte
granularity. This RFC rationalizes the handling of these two sizes in the
Zircon system interface.

## Motivation

The VMO-related parts of the Zircon system interface are inconsistent in their
handling of the VMO *size* and *content size*. This inconsistency is a product
of how these interfaces evolved. At various points in time, we attempted to
express the size of each VMO at byte granularity, at page granularity, and,
prior to this RFC, at both byte and page granularity. This inconsistency makes
it difficult to evolve the Zircon system interface, for example to add more
sophisticated paging interfaces.

## Stakeholders

Who has a stake in whether this RFC is accepted? (This section is optional but
encouraged.)

_Facilitator:_

 * hjfreyer@google.com

_Reviewers:_

 * cpu@google.com
 * csuter@google.com
 * jamesr@google.com
 * rashaeqbal@google.com

_Consulted:_

 * adanis@google.com
 * dworsham@google.com
 * sagebarreda@google.com

_Socialization:_

The proposal in this RFC emerged from a series of discussions between the local
storage, the virtual memory manager, and the architecture teams. After
considering a wide range of approaches, this group decided to propose some
fairly modest changes to the Zircon system interface, described below.

## Requirements

 * The system MUST continue to function after making these changes. For example,
   files MUST have byte granularity lengths because vast amounts of code
   depends on having file lengths at byte granularity.
 * The design MUST be efficient to implement in the kernel and in hardware. For
   example, memory mappings MUST be at page granularity because that is what
   hardware supports.
 * The design SHOULD minimize the changes to any public interfaces, either
   to the kernel or to SDK libraries. For example, FDIO has a public interface
   that converts a VMO into a file descriptor without any additional contextual
   information, which SHOULD be preserved. However, small changes to public
   interfaces are permissible if migrating clients is tractible.
 * The design SHOULD be as simple as possible. The virtual memory manager is
   already a significant source of complexity in the system. We need to be
   cautious whenever we add more complexity to this part of the system.

The key words "MUST", "MUST NOT", "REQUIRED", "SHALL", "SHALL NOT", "SHOULD",
"SHOULD NOT", "RECOMMENDED", "MAY", and "OPTIONAL" in this document are to be
interpreted as described in
[IETF RFC 2119](https://tools.ietf.org/html/rfc2119).

## Design

Conceptually, a VMO is a sparse collection of pages of memory. VMOs are used
for many purposes throughout the system, including storing and sharing data,
which exists at byte-granularity. There are various types of VMOs, depending
on the kind of memory stored in the VMO. For example, a VMO might contain
virtual memory that is backed by a pager or physical memory, which has a more
concrete relationship to hardware resources.

VMOs have a *size*, which represents the maximum amount of memory that can be
stored in the VMO. This size is always an even multiple of the *page size*
because VMOs store data at page granularity. Clients can learn the size of a
VMO using the `zx_vmo_get_size` function. If the VMO was created with the
**ZX_VMO_RESIZABLE** option, clients can change the size of the VMO with the
`zx_vmo_set_size` function, assuming they have a handle with
**ZX_RIGHT_RESIZE**. The vast majority of VMO operations interact with this
size.

VMOs also have a *stream size*, which represents the number of bytes in the
data stream stored within the VMO, if any. The stream size is initialized
when the VMO is created, typically based on an argument to the function that
creates the VMO, and is not necessarily
an even multiple of the page size. The stream size is never larger than
the VMO size because the data stream stored within the VMO cannot be larger
than the maximum amount of memory that can be stored in the VMO. Clients can
learn the stream size of a VMO using the `zx_vmo_get_steam_size` function. If
the VMO is suitable for storing data streams, clients can change the stream
size of the VMO with the `zx_vmo_set_stream_size` function, assuming they have
a handle with **ZX_RIGHT_WRITE**. The `zx_vmo_set_size` function does not modify
the stream size, other than to enforce the invariant that the stream size is
never larger than the size of the VMO itself. Typically, clients interact with
the stream size using the `zx_stream_*` functions on a stream object associated
with the VMO. The stream size is shared between all stream objects associated
with the same VMO.

## ZX_VMO_UNBOUNDED

When a client creates a VMO with `zx_vmo_create`, the *size* argument is used to
initialize both the size and the stream size of the VMO. The size of the VMO is
initialized to the *size* rounded up to the nearest page boundary. The stream
size of the VMO is initialized to the *size* without any rounding.

This RFC introduces the **ZX_VMO_UNBOUNDED** option for `zx_vmo_create` and
`zx_pager_create_vmo` for creating a VMO with a large size. If the client
supplies this option, the size of the VMO is initialized to the maximum
possible value instead of being initialized based on the *size* argument. The
**ZX_VMO_UNBOUNDED** option is useful for situations in which the size of the
VMO does not serve a useful purpose, such as for VMOs used by pager-backed file
systems and for VMOs that back the C runtime heap. For this reason, the
**ZX_VMO_UNBOUNDED** cannot be used with the **ZX_VMO_RESIZABLE** option.

## ZX_VM_FAULT_BEYOND_STREAM_SIZE

This RFC introduces the **ZX_VM_FAULT_BEYOND_STREAM_SIZE** option for
`zx_vmar_map`. If the client supplies this option, memory accesses to the
mapping beyond the last page containing the data stream within the VMO will
fault. The mapping can still be used to read and write memory outside the data
stream, but only up to the next page boundary because memory mappings exist at
page granularity. When a VMO with these kinds of mappings is resized, Zircon
will remove the page table entries for pages beyond the data stream so that
future accesses to those pages will generate faults.

The **ZX_VM_FAULT_BEYOND_STREAM_SIZE** option is useful for implementing `mmap`
semantics that match other POSIX-like operating systems (e.g., Linux and
OpenBSD). In those operating systems, memory accesses beyond the last page that
overlaps the content of a memory-mapped file generate faults.

## Dirty ranges

A future RFC for `zx_pager_query_dirty_ranges` will likely add an option to
support querying dirty ranges in the data stream of the VMO, as opposed to the
current default of querying dirty ranges in the entire VMO.

## ZX_PROP_VMO_CONTENT_SIZE

This RFC *deprecates* **ZX_PROP_VMO_CONTENT_SIZE**. Clients should use
`zx_vmo_get_stream_size` and `zx_vmo_set_stream_size` instead. Zircon will
retain legacy compatibility for **ZX_PROP_VMO_CONTENT_SIZE** for the
foreseeable future. However, in addition to **ZX_RIGHT_GET_PROPERTY** and
**ZX_RIGHT_SET_PROPERTY**, getting and setting **ZX_PROP_VMO_CONTENT_SIZE**,
will require the same rights as `zx_vmo_get_stream_size` and
`zx_vmo_set_stream_size`, respectively (i.e., **ZX_RIGHT_WRITE**
for setting **ZX_PROP_VMO_CONTENT_SIZE**).

## Implementation

This section describes the detailed changes to each Zircon syscall.

### zx_pager_create_vmo

As today, The size of the VMO is initialized to the *size* rounded up to the
nearest page boundary. The stream size of the VMO is initialized to the *size*
without any rounding.

If the client supplies the **ZX_VMO_UNBOUNDED** option, this operation creates a
VMO whose size is initialized to the maximum possible value.

If the client supplies both the **ZX_VMO_UNBOUNDED** and the
**ZX_VMO_RESIZABLE** options, this operation returns **ZX_ERR_INVALID_ARGS**.

### zx_pager_query_vmo_stats

No changes. However, this operation can return potentially surprising results
if the VMO contains modifications beyond the data stream. In a future RFC, we
expect to add an option to `zx_pager_query_dirty_ranges` to restrict the query
to the data stream.

### zx_stream_create

If the *vmo* argument refers to a VMO object created with
`zx_vmo_create_contiguous` or `vmo_create_physical`, this operation returns
**ZX_ERR_WRONG_TYPE**.

### zx_stream_writev

If this operation attempts to write beyond the end of the data stream, this
operation will increase the size of the data stream, analogously to how
`zx_vmo_set_stream_size` changes the size of the data stream. If the data
stream cannot be increased, for example because the new *stream size* would
exceed the *size* of the VMO, then the resize operation fails. The
documentation for `zx_stream_writev` defines precisely how this situation is
handled, but, roughly, the `writev` operation succeeds with a partial write if
the operation was able to write any data into the VMO and propagates the error
otherwise.

These semantics are different from the previous semantics for this operation.
Prior to this RFC, the `zx_stream_writev` would attempt to change the *size* of
the VMO if the desired *stream size* would exceed the *size* of the VMO. After
this RFC, the *size* of the VMO is never changed by stream operations.

### zx_vmo_create

As today, The size of the VMO is initialized to the *size* rounded up to the
nearest page boundary. The stream size of the VMO is initialized to the *size*
without any rounding.

If the client supplies the **ZX_VMO_UNBOUNDED** option, this operation creates a
VMO whose size is initialized to the maximum possible value.

If the client supplies both the **ZX_VMO_UNBOUNDED** and the
**ZX_VMO_RESIZABLE** options, this operation returns **ZX_ERR_INVALID_ARGS**.

### zx_vmo_create_child

The changes to this operation are specific to the mode:

 * **ZX_VMO_CHILD_SNAPSHOT** - If the *size* is not a multiple of page size,
   this operation returns **ZX_ERR_INVALID_ARGS**. Previously, this operation
   supported non-aligned size values, but that behavior was potentially
   dangerous because the child VMO actually has access to a integral number of
   pages from the parent VMO.

   The size and the stream size of the child VMO are both initialized to the
   *size* argument. The stream size of the child can be changed independently
   from that of the parent.

 * **ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE** - Similar behavior as with
   **ZX_VMO_CHILD_SNAPSHOT**.

 * **ZX_VMO_CHILD_SLICE** - Similar behavior as with
   **ZX_VMO_CHILD_SNAPSHOT**.

 * **ZX_VMO_CHILD_REFERENCE** - The *stream size* of the child is initialized
   to the *stream size* of the parent.

### zx_vmo_create_contiguous

If the *size* is not a multiple of page size, this operation returns
**ZX_ERR_INVALID_ARGS**.

The size of the VMO is initialized to *size*.

The stream size of the VMO is initialized to zero and cannot be modified.

### vmo_create_physical

If the *size* is not a multiple of page size, this operation returns
**ZX_ERR_INVALID_ARGS**.

The size of the VMO is initialized to *size*.

The stream size of the VMO is initialized to zero and cannot be modified.

### zx_vmo_get_size

No changes.

### zx_vmo_set_size

If the *size* argument is not a multiple of page size, this operation returns
**ZX_ERR_INVALID_ARGS**.

Overwrites the size of the VMO to the *size* argument and zeros (i.e.,
discards) any pages beyond this point. If the stream size is greater than the
new VMO size, truncates the stream size to match the new VMO size.

This behavior differs from the previous behavior of this operation, which could
be used to increase the content size of the VMO. To increase the stream size
beyond the current size if the VMO, first increase the size of the VMO with
`zx_vmo_set_size` and then increase the stream size with
`zx_vmo_set_stream_size`.

### zx_vmo_get_stream_size

Returns the *stream size* of the VMO.

### zx_vmo_set_stream_size

If the *size* argument is greater than the size of the VMO, this operation
returns **ZX_ERR_OUT_OF_RANGE**.

Overwrites the stream size of the VMO to the *size* argument and zeroes from the
smaller of *size* and the previous stream size to the end of the VMO. This
differs from the previous behavior of setting the content size where zeroing
would only happen from the new *size* to the end of the VMO. This difference
means that when the stream size is increased the newly revealed range is zeroed.

This behavior change ensures that newly revealed content is always zero even if
the VMO beyond the stream size had been modified via other means, such as a
direct memory mapping.

If this operation shrinks the data stream, any pages that no longer overlap
with the data stream mapped with the **ZX_VM_FAULT_BEYOND_STREAM_SIZE** option
are unmapped.

In both the growing and shrinking cases, this operation can cause copy-on-write
and user pager-backed VMOs to commit the last page that overlaps the data
stream to store zeros in that page.

The *handle* must have **ZX_RIGHT_WRITE**.

### zx_vmo_op_range

No changes.

### zx_vmo_read

No changes.

This operation interacts with the *size* of the VMO rather than the *stream
size*, which means this operation can read data beyond the data stream.

### zx_vmo_write

No changes.

This operation interacts with the *size* of the VMO rather than the *stream
size*, which means this operation can write data beyond the data stream.

### zx_vmar_map

If the client supplies the **ZX_VM_FAULT_BEYOND_STREAM_SIZE** option, memory
accesses to the mapping beyond the last page containing the data stream within
the VMO will fault. This option requires the **ZX_VM_ALLOW_FAULTS** option.

## Performance

This RFC should have no impact on performance. The main changes relate to how
Zircon reports VMO metadata. The semantics of each operation have been chosen
to avoid performance challenges in the implementation.

## Ergonomics

The conceptual model for VMOs is complex because some operations deal with the
VMO as a collection of memory pages whereas other operations interact with the
data stream stored within the VMO. The page-oriented operations operate at
page-granularity whereas the stream-oriented operations operate at
byte-granularity. This complexity arises because the virtual memory hardware in
commodity CPUs is designed around pages but the *data* that clients store in
memory is designed around bytes.

This RFC attempts to disambiguate these concepts by more clearly separating
the page-based and stream-based operations. For example, `zx_vmo_set_size` is
a page-based operation and therefore requires a page-aligned size whereas
`zx_vmo_set_stream_size` is a stream-based operation and therefore supports
byte-aligned sizes.

Some cases are subtle, such as `zx_vmo_read` and `zx_vmo_write`. As reasonable
client might assume these operations interact with the data stream because they
read and write data. However, we have chosen to make these operations mirror
the semantics available through the *load* and *store* operations on mapped
memory, which means they interact with the page-oriented size of the VMO.

This situation is overconstrained, especially given the path-dependent nature
of iterating on the design of a system with a large amount of existing
software. We have done our best to find the most ergnomic solutions given the
design constraints.

## Backwards Compatibility

This change creates some backwards compatibility risks. For example,
`zx_vmo_create_child`, `zx_vmo_create_contiguous`, and `zx_vmo_create_physical`
now require page-aligned sizes and setting **ZX_PROP_VMO_CONTENT_SIZE**
requires an additional right, both of which will break some existing code. We
believe we can migrate that code to respect these new constraints, but we might
need to relax these constraints if we fail to perform these migrations.

The other changes in this RFC are unlikely to cause backwards compatibility
issues. In most cases, the existing semantics are preserved, just explained in
different terms.

## Security considerations

The main security risk in this RFC is that data can exist in VMOs beyond the
data stream within a VMO. That behavior is likely to be surprise to many
developers, and surprises are always security risks. However, typical usage
of these operations will result in that data always being zeroes and this
risk exists in other widely used operating systems.

## Privacy considerations

In theory, data stored beyond the data stream within a VMO could be a privacy
issue if that data is disclosed unexpectedly. However, in normal operation,
the data beyond the data stream of a VMO will be zeroes.

## Testing

We will add appropriate Zircon core tests to validate all the changes to
syscall semantics.

## Documentation

The syscall documentation will be updated to reflect the new semantics as those
changes are implemented.

## Drawbacks, alternatives, and unknowns

We have tried various alternatives over the course of the project, none of
which have worked well. Ideally, VMOs would have a consistent mental model
built around either a byte-granularity or a page-granularity size. However,
there are strong use cases for byte granularity because the industry has
standardized on eight-bit bytes as the size granularity for data and there
are strong hardward-based constraints for dealing with memory at page
granularity. As a result, neither extreme is a tenable design point.

In practice, the primary alternative to this RFC is to do nothing and leave
the current dual size model for VMOs in place. However, we believe the modest
changes in this RFC are an improvement on the current model. We considered
larger changes to the model, but, in each case, we uncovered use cases or
design constraints that pushed the design back towards this more modest
approach.

## Prior art and references

There is a huge amount of prior art in this area. The prior art that is
primarily relevant is the way files interact with memory mappings in other
popular operating systems. In every case that we studied, files had byte
granularity whereas memory mappings had page granularity. We especially
studied how memory mappings of the last page of a file (e.g., that extends
beyond the end of the file's content) operate. The
**ZX_VM_FAULT_BEYOND_STREAM_SIZE** flag is designed to let us replicate the
semantics we observed in Windows, macOS, and Linux. However, we did not select
these semantics as our default because `zx_vmar_map` does not allow faults by
default. We expect most cross-platform code that constructs memory mappings
(e.g., `mmap`) will supply this flag to align semantics across operating
systems.
