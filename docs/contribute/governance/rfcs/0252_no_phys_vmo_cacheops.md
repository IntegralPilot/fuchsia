<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0252" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Summary

Remove the ability to perform cache operations directly on a physical VMO.

## Motivation

Cache operations on physical VMOs do not work reliably today as they rely on the
physmap. Given that physical VMOs can only be manipulated with a mapping, they
can always use the existing `zx_cache_flush` operation instead.

Physical VMOs typically represent non RAM regions of the physical address space,
such as MMIO registers. Performing cache operations is at best nonsensical, or
at worst a fatal error depending on the architecture and hardware. Although
physical VMOs already require a restricted resource to create, minimizing the
ways users can crash the kernel is still a good thing.

## Stakeholders

_Facilitator:_

cpu@google.com

_Reviewers:_

mcgrathr@google.com, hansens@google.com, jbauman@google.com

_Consulted:_

_Socialization:_

Discussed among the Zircon kernel/VM teams.

## Design

As `zx_cache_flush` already exists and is implemented there is nothing new that
needs to be built for this proposal. The proposal therefore is simply to remove
support for the following operations on physical VMOs, i.e. those created by
`zx_vmo_create_physical`:

 * `ZX_VMO_OP_CACHE_SYNC`
 * `ZX_VMO_OP_CACHE_INVALIDATE`
 * `ZX_VMO_OP_CACHE_CLEAN`
 * `ZX_VMO_OP_CACHE_CLEAN_INVALIDATE`

These operations on non physical VMOs, i.e. those created via `zx_vmo_create`,
`zx_vmo_create_contiguous`, `zx_vmo_create_child` or `zx_pager_create_vmo`, will
be unchanged and behave as before.

This results in divergent APIs between physical VMOs and paged VMOs which is not
ideal, as code may need to tolerate being given either kind. However,
physical VMOs are not commonly used and, as removing cache operations from all
VMOs is presently infeasible, code that needs to support either kind of VMO will
have to ensure it has a mapping available.

## Implementation

There are not many usages of the physical VMO cache operations in tree, as most
locations that need a cache operation are already using the preferred
`zx_cache_flush`. Of the existing usages of VMO cache operations there is always
an existing mapping that could instead be used for a `zx_cache_flush`, making a
migration trivial.

The implementation plan then is as follows:

 1. Migrate any existing VMO cache operation usages that touch physical VMOs.
 2. Disable cache operations on physical VMOs, and have them return
    `ZX_ERR_NOT_SUPPORTED`.

## Performance

Performing cache operations using `zx_cache_flush` should be more efficient than
doing a syscall, and so this change should generally be a net performance
improvement. The exception to this will be any scenarios where a mapping needs
to be created, however physical VMOs are not commonly used and no such case
exists today. This decision can be revisited in the future if needs change.

## Ergonomics

Most cache operations today are already performed with `zx_cache_flush` as it is
easier and more convenient to use where possible, and from an ergonomic
perspective would always be preferred over the VMO operations. It is, however,
far less ergonomic if a mapping does not already exist.

## Backwards Compatibility

This is an API breaking change, however cache operations are done by a small
subset of users, typically drivers and similar, and even fewer have physical
VMOs and so should not be a difficult migration for existing code.

## Security considerations

This proposal both removes code and logic from the kernel and causes user
controlled cache operations to not happen against potential device memory while
running in privileged mode. It is therefore neutral to slightly positive from a
security point of view.

## Drawbacks, alternatives, and unknowns

The assumption of this proposal is that users of cache operations on physical
VMOs either:

 * Already have a mapping and switching to performing cache operations on it is
   therefore a performance improvement with no downside.
 * Do not have a mapping but could create one in a non performance critical
   initialization section.
 * Do not have a mapping, are not performance critical, and can create a
   temporary mapping to perform the cache operation.

It is possible that there could be a user that:

 * Does not have a mapping.
 * Does not have a non performance critical initialization point where a mapping
   could be created.
 * Is otherwise performance critical when performing cache operations.

With these assumptions on existing users, and the unsupported potential user, in
mind we can examine some alternatives to completely removing cache operations.

### Remove for all VMOs

Removing cache operations for all VMOs would provide a uniform API, which would
prevent scenarios where code might fail if given a physical VMO where previously
it had only expected paged VMOs.

Unfortunately there are common patterns where a service or driver might be given
a VMO and it has no need to create a mapping prior to pinning and passing to the
underlying hardware. In this scenario, data written by the client might need to
be cleaned to memory, via a cache operation. However, until the VMO is pinned
its underlying pages might be changed by the kernel, rendering any prior cache
clean insufficient. As only the driver, not the client, can pin the memory, this
means the driver must perform the cache operation after pinning. Requiring a
mapping to do so would add unnecessary overhead to the driver in this case.

### Separate kernel mapping for physical VMOs

Instead of relying on the physmap, the kernel could create a separate mapping
for every physical VMO and perform cache operations on this mapping instead.

This does resolve the direct issue of physmap usage, unblocks all related kernel
changes, and ensures that the hypothetical performance critical user without a
mapping is not impacted.

The downsides are:

 * Additional page tables need to be allocated to keep this mapped in, wasting
   memory if the user has a mapping anyway. Physical VMOs can potentially be to
   quite large ranges.
 * Increases kernel complexity and continues allowing users to perform cache
   operations on invalid ranges that could fault the kernel.

#### Temporary kernel mapping

The overhead of page tables could be alleviated by performing a temporary
mapping just in the cache operation code path. However, this has little benefit
over users performing the temporary mapping themselves, and so does not actually
improve on the original proposal.
