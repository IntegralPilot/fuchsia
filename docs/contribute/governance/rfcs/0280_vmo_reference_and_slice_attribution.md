<!-- Generated with `fx rfc` -->
<!-- mdformat off(templates not supported) -->

{% set rfcid = "RFC-0280" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}

# {{ rfc.name }}: {{ rfc.title }}

{# Fuchsia RFCs use templates to display various fields from \_rfcs.yaml. View
the #} {# fully rendered RFCs at
https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}

<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Problem Statement

[References](0204_vmo_reference_child.md) and slices are VMO child types that
forward operations, such as reads and writes, to the parent VMO. Operations
behave as though they were performed on a duplicate handle to the parent VMO.

Currently, references and slices report zero attributed bytes for all fields
when queried with [zx_object_get_info](/reference/syscalls/object_get_info.md).

In addition, it is impossible to query whether a VMO is a reference or slice,
which makes it difficult to tell if zero attributed bytes means there is no
content. Users of the API rely on hacks to detect and attribute these clone
types.

This scheme has caused some problems.

### Fxfs

Fxfs uses references to hand out handles to blobs and mutable files. This is
problematic because it's impossible to retrieve information about clients that
the file is being held open for when using the existing API. To attribute for
fxfs, memory monitor currently relies on a hack which considers all VMOs with 0
total bytes as references.

### IOBuffers

[IOBuffers](0218_io_buffer.md) use references for tracking peers on an endpoint.
In the IOBuffer implementation, the root VMO is in the kernel so attribution
cannot be queried form userspace. This case also includes nested references,
which further complicates things.

### Orphaned memory

If a VMO is dropped while it has only reference and slice clones, the VmCowPages
of the original VMO will remain alive, but the pages won't be attributed to any
VMO. This will be considered as "orphaned memory" by memory monitor due to the
API limitation that it is impossible to retrieve any information about the
original VMO or its clones.

## Summary

This RFC proposes a change in memory attribution for reference and slice
children of VMOs via `zx_object_get_info`. Reference and slice children will now
report memory as though the query was performed on the parent. A slice will only
report bytes in the range it can see.

Additionally, two new flags will be added to `zx_info_vmo::flags` so the user
can query if the VMO is a reference,`ZX_INFO_IS_REFERENCE`, or a slice,
`ZX_INFO_VMO_IS_SLICE`.

These changes should give users of the API enough information to manage
reference attribution at their discretion.

## Stakeholders

_Facilitator:_

jamesr@google.com

_Reviewers:_

etiennej@google.com, rashaeqbal@google.com

_Consulted:_

adanis@google.com

_Socialization:_

A document outlining the problems with the existing attribution scheme, the
proposed solutions and alternative solutions that were considered was sent to
_fuchsia-memory-wg_ and _zircon-discuss_ mailing lists for comment.

## Requirements

In order for memory-monitor and general memory accounting to function, including
emulating linux memory attribution APIs, it should still be possible to
calculate these standard metrics from the information provided in
`zx_info_vmo_t`:

- Total memory (equivalent to RSS/resident set size)
  - "Total memory this VMO references"
- Private memory (equivalent of USS/unique set size)
  - "Memory that would be freed if this VMO was destroyed"
- Scaled memory (equivalent to PSS/proportional set size)
  - "This VMO's share of total memory"
  - Must sum to total system memory usage

## Design

Attribution information returned from
[zx_object_get_info](/reference/syscalls/object_get_info.md) will be changed
such that references and slices will report bytes as though they are the parent.
Two flags will be added to `zx_info_vmo::flags` so the user can query if the VMO
is a reference,`ZX_INFO_IS_REFERENCE`, or a slice, `ZX_INFO_VMO_IS_SLICE`.

This allows users of the API to make decisions on how to attribute bytes for
references. Total memory could be reported as though the reference was the
parent. The new flags can be used to return 0 for private memory and correctly
calculate total memory.

### Reference Example

An example of the attribution change with a reference, consider a VMO with two
committed pages and one reference child. This is how queries to the API will
change, assuming page size is 4096.

Existing attribution:

```c++
zx_info_vmo_t info;
reference.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr)

info.committed_bytes = 0
info.populated_bytes = 0
info.committed_private_bytes = 0
info.populated_private_bytes = 0
info.committed_scaled_bytes = 0
info.populated_scaled_bytes = 0
info.committed_fractional_scaled_bytes = 0
info.populated_fractional_scaled_bytes = 0
```

Proposed attribution:

```c++
zx_info_vmo_t info;
reference.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr)

info.committed_bytes = 8192
info.populated_bytes = 8192
info.committed_private_bytes = 8192
info.populated_private_bytes = 8192
info.committed_scaled_bytes = 8192
info.populated_scaled_bytes = 8192
info.committed_fractional_scaled_bytes = 0
info.populated_fractional_scaled_bytes = 0

info.flags & ZX_INFO_IS_REFERENCE = true;
info.flags & ZX_INFO_IS_SLICE = false;

```

### Slice Example

If we had a VMO with two committed pages with a one-page slice, this is how the
new API will report attribution, assuming page size is 4096. It will behave as
though attribution was queried on the parent for the range the slice can see.

Proposed attribution:

```c++
zx_info_vmo_t info;
slice.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr)

info.committed_bytes = 4096
info.populated_bytes = 4096
info.committed_private_bytes = 4096
info.populated_private_bytes = 4096
info.committed_scaled_bytes = 4096
info.populated_scaled_bytes = 4096
info.committed_fractional_scaled_bytes = 0
info.populated_fractional_scaled_bytes = 0

info.flags & ZX_INFO_IS_REFERENCE = false;
info.flags & ZX_INFO_IS_SLICE = true;
```

## Implementation

The kernel implementation of this change will be simple, but requires
coordination with memory monitor to ensure that they don't report incorrect
values from references and slices.

## Performance

This change will introduce a walk up the cow-pages tree where we would have
otherwise returned zero, so it's possible that `zx_object_get_info` will be
slower when querying some references.

## Security considerations

By querying '\*\_scaled' fields or comparing private bytes to non-private from a
reference, side channel information about copy-on-write clones of the original
VMO could be inferred that would have been impossible with the old attribution
scheme. This will not cause any problems with current users of references.

This is not expected to cause problems in future uses, as having a reference is
conceptually similar to having an additional handle to the VMO.

## Privacy considerations

N/A

## Testing

Any existing tests that query attribution on references or slices will need to
be updated.

## Documentation

The doc for [zx_object_get_info](/reference/syscalls/object_get_info.md) syscall
will be updated.

## Drawbacks, alternatives, and unknowns

One drawback of this design is the possibility of introducing double-counting
bugs with nested references, if the parent ID was being used to determine the
VMO that owned the pages.

An alternate approach was to extend sharing to include references and slices
along with copy-on-write children. This would have been a larger change to the
system as it would change attribution for copy-on-write clones, and the
calculation of USS and PSS would need to be modified for all VMOs.
