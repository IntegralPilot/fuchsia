<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0262" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Problem Statement

The existing [`ZX_INFO_KMEM_STATS`](https://fuchsia.dev/reference/syscalls/object_get_info#zx_info_kmem_stats)
query is missing fields to report newer kernel memory usage types, creating
confusion in how to interpret the existing fields, and the existing fields
themselves have confusing or minimal documentation. This RFC seeks to address
both of these problems.

This RFC does not seek to change the style of reporting that
`ZX_INFO_KMEM_STATS` does or otherwise support additional kinds of memory usage
queries.

## Summary

Expand the structure returned by the `ZX_INFO_KMEM_STATS` to include counting
for pages in states that did not exist when the query was originally written and
include the fields from the [`ZX_INFO_KMEM_STATS_EXTENDED`](https://fuchsia.dev/reference/syscalls/object_get_info#zx_info_kmem_stats_extended)
query, deprecating it in the process. Update the documentation for both the new
and existing fields for clarity and correctness.

## Stakeholders

_Facilitator:_

davemoore@google.com

_Reviewers:_

maniscalco@google.com, mcgrathr@google.com

_Consulted:_

plabatut@google.com

_Socialization:_

A CL to implement this RFC was iterated on with the target reviewers.

## Design

Existing kernel struct evolution and versions strategies will be used to provide
a new version of the `ZX_INFO_KMEM_STATS` query and deprecate the
`ZX_INFO_KMEM_STATS_EXTENDED` query. The new version of the `ZX_INFO_KMEM_STATS`
struct will include fields for the missing `vm_page_state`, these these are:
`FREE_LOANED`, `CACHE`, `SLAB` and `ZRAM` states, and all the fields from
`ZX_INFO_KMEM_STATS_EXTENDED`.

Documentation changes are focused on clarifying some of the following items:

*   Memory in these states is not in use by the kernel, but is the kernel's
    interpretation of system memory usage.
*   Increased specificity on memory being 'free' as memory can be both
    unallocated, but only available for certain kinds of allocations.
*   How to interpret memory in VMOs that is reported in multiple states and when
    that reporting overlaps or not.

## Implementation

This will be landed as two CLs:

 1. Update the `ZX_INFO_KMEM_STATS` query and all documentation with a new
    versioned topic number and struct.
 2. Deprecate `ZX_INFO_KMEM_STATS_EXTENDED` once users have switched to
    `ZX_INFO_KMEM_STATS`.

The deprecation will leave the query implemented in the kernel, preserving
binary compatibility.

## Performance

Expanding the fields in `ZX_INFO_KMEM_STATS` will make the query take more time,
especially with adding the `ZX_INFO_KMEM_STATS_EXTENDED` fields, but this cost
should still be negligible relative to existing cost of the syscall. Primary
existing user, memory_monitor, sometimes performs both queries anyway and in
those cases a single syscall will improve performance.

Actual syscall performance cost and potential impact on memory_monitor should
still be validated.

## Backwards Compatibility

The struct evolution strategy preserves binary compatibility for any prebuilt
artifacts, and since fields are only added source compatibility should be
preserved in most cases. It is possible that the existing info struct is being
used in a way that adding fields breaks, in which case the old version of the
struct and query can be used until that usage site can be updated.

## Documentation

Updating the documentation of the `ZX_INFO_KMEM_STATS` query is an explicit goal
of this RFC and such changes will be part of any CLs.

## Drawbacks, alternatives, and unknowns

Instead of deprecating the `ZX_INFO_KMEM_STATS_EXTENDED` query, it could be
maintained and its fields not merged into `ZX_INFO_KMEM_STATS`, preventing any
performance concerns. Although the `ZX_INFO_KMEM_STATS_EXTENDED` query was
intended to be a place for information deemed too expensive for
`ZX_INFO_KMEM_STATS`, this information is, due to kernel evolution, no longer
expensive to gather and has no benefit being separated.
