<!--
// LINT.IfChange
-->
<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0223" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Summary

We propose adding a new syscall `zx_vmo_transfer_data` that allows callers to
move pages from one VMO to another. It can be thought of as an efficiency
improvement that allows data moves without incurring the overhead of a copy.

## Motivation

Past analyses of Fuchsia performance have found that the deep, hierarchical CoW
clone chains of Bootfs were causing long search times when looking up a page.
This syscall will reduce this overhead by replacing the clone chains with
"spliced" VMOs containing individual entries of Bootfs.

While Bootfs is the current motivation, there may be other future cases where
the ability to move pages can improve performance.

## Stakeholders

Who has a stake in whether this RFC is accepted? (This section is optional but
encouraged.)

_Facilitator:_ cpu@

_Reviewers:_ rashaeqbal@, jamesr@

_Consulted:_ mcgrathr@, maniscalco@, adanis@, eieio@

_Socialization:_ This proposal was socialized as a one pager to the Zircon
team.

## Design

We will add the following syscall to the Zircon syscall API:

```
zx_status_t zx_vmo_transfer_data(zx_handle_t dst_vmo,
                                 uint32_t options,
                                 uint64_t offset,
                                 uint64_t length,
                                 zx_handle_t src_vmo,
                                 uint64_t src_offset);
```
where:

* `dst_vmo` is a handle to the destination VMO. This handle must have
`ZX_RIGHT_WRITE`.
* `options` is a currently unused field that allows for expansion of the API in
the future.
* `offset` is the offset at which pages are moved into the destination.
* `length` is the number of bytes to move to the destination.
* `src_vmo` is a handle to the source VMO. This handle must have
`ZX_RIGHT_READ` and `ZX_RIGHT_WRITE`.
* `src_offset` is the offset at which pages are retrieved from the source.

This syscall will move the pages in `[src_offset, src_offset + length)`
from `src_vmo` to `[offset, offset + length)` in `dst_vmo`. It is functionally
equivalent to a `memmove` from `src_vmo` to `dst_vmo` followed by a decommit of
the associated pages in `src_vmo`. However, the mechanism by which this is
achieved is different; the backing pages are actually moved between VMOs
instead of copying data. This allows for much better performance. Despite this
different mechanism, this syscall presents the same semantics as `memmove`, in
that providing overlapping source and destination regions is supported.

The reader may be wondering why the syscall is called `zx_vmo_transfer_data`
and not `zx_vmo_transfer_pages` if it moves pages. This was a deliberate choice
made in case we wanted to relax the strict page alignment requirement in the
future. For example, we may want to support the use case in which a user
requests a page and a half to be transferred. In this case, we would move the
first page and then copy out the remaining half page. This would still be more
efficient than a naive copy by the user, as it would allow us to bypass zeroing
the destination pages. Note that the initial implementation does not support
this use case; it is mentioned here only to provide context on the naming
choice.

Existing pages in the destination range will be overwritten. All mappings of
the destination VMO will also see the new contents. If the destination VMO has
children, the type of the child will influence the contents the child sees.
Children of type `ZX_VMO_CHILD_SNAPSHOT` and `ZX_VMO_CHILD_SNAPSHOT_MODIFIED`
will continue to see the old contents. Children of all other types will see the
new contents. Once the move is complete, the pages in `src_vmo` will be zeroed
out.

The move can fail for a variety of reasons. Refer to the errors section
below for a complete enumeration of possible error codes and the scenarios that
return them.

If the move fails, then any number of pages in the `src_vmo` VMO may have been
moved to `dst_vmo`. We make no guarantees as to exactly how much data was moved.
However, we can guarantee that the call will succeed when the following
conditions are met:

1. None of the conditions that would result in the errors listed below are met.
1. The `src_vmo` and `dst_vmo` are not modified by any other threads while this
operation is running.

A "modification" in this context refers to a write/resize/pin on either the VMO
directly or on a reference to the VMO (e.g. slices, reference children, etc.).
Modifying a parent, child, or sibling of any kind of snapshot should not result
in any error, although depending on the snapshot you may get write tearing.
Write tearing could occur if you manipulate the parent of a
`SNAPSHOT_AT_LEAST_ON_WRITE` VMO as the actual transfer has no promised
atomicity. Note that in the case of transferring pages from a SNAPSHOT child we
may need to perform copies, i.e. allocate new pages, if that particular page has
not yet been copy-on-written.

Here are the errors this syscall can return and what they mean:

* `ZX_ERR_BAD_HANDLE`:  `src_vmo` or `dst_vmo` is not a valid VMO handle.
* `ZX_ERR_INVALID_ARGS`: `offset`, `length`, or `src_offset` is not page
    aligned. As discussed earlier, we may want to remove this constraint in the
    future.
* `ZX_ERR_ACCESS_DENIED`: `src_vmo` does not have `ZX_RIGHT_WRITE` and
    `ZX_RIGHT_READ`, or `dst_vmo` does not have `ZX_RIGHT_WRITE`.
* `ZX_ERR_BAD_STATE`: Pages in the specified range in `src_vmo` or `dst_vmo`
    are pinned.
* `ZX_ERR_NOT_SUPPORTED`: Either `src_vmo` or `dst_vmo` are physical,
    contiguous, or pager-backed VMOs. We may be able to support pager-backed
    VMOs in the future.
* `ZX_ERR_OUT_OF_RANGE`: The specified range in `dst_vmo` or `src_vmo` is
    invalid.
* `ZX_ERR_NO_MEMORY`: Failure due to lack of memory.

## Implementation

This should be a relatively straightforward set of CLs, as we have an existing
syscall `zx_pager_supply_pages` that does something very similar for pager
backed VMOs. We can therefore reuse a lot of the code backing that syscall.
However, we will need to make several changes to support this new use case:

1. `zx_pager_supply_pages` validates that the provided VMO is backed by the
given pager object. This will need to be removed in our new syscall.
1. `SupplyPages`, the function `zx_pager_supply_pages` uses to insert pages
into a VMO, assumes the existence of a pager, referred to in code as a
`page_source_`. We must remove this assumption by adding NULL checks before
operating on the `page_source_` and removing any assertions on its existence.
1. `SupplyPages` also decompresses any compressed pages and adds markers before
splicing the pages into the destination, as it expects the destination to be
pager-backed. This is not required for Anonymous VMOs, so we'll need to make
it conditional on whether the destination is pager-backed or not.
1. `SupplyPages` currently skips any page that exists in the destination, but
still frees the page in the source. As mentioned earlier, we'll change this
to always overwrite the destination.
1. `SupplyPages` does not currently account for VMOs with parents. In most
cases, this is not a problem. However, in cases where the destination is a
child of type `ZX_VMO_CHILD_SNAPSHOT`, we'll need to update the split bits
in the parent to signify that the VMO has diverged from the hidden parent.

## Performance

We expect this to significantly improve VMO page lookup performance in Bootfs.
Concretely, switching from the CoW clone chains in use today to moving pages
with this syscall should result in a ~20% improvement in bootfs lookup.
Note that we could get a similar bootfs lookup improvement by creating copies
of VMOs instead of moving pages like this syscall suggests. However,
this approach does regress startup time of the system by up to 70% (depending
on the hardware target we're running on). Using the approach proposed by this
RFC recoups most of this regression.

## Backwards Compatibility

We do not aim to remove the existing `zx_pager_supply_pages` syscall, so we do
not anticipate any issues with backwards compatibility.

## Security considerations

We do not anticipate any security ramifications from this proposal, as
the new operation is intended to be functionally equivalent to existing
operations (memcpy() + uncommit), but with better performance. It is not
intended to allow a process to do things it couldn't do already.

Nevertheless, it increases attack surface. If there are exploitable bugs in the
syscall implementation, it's possible that any process could exploit them.

## Privacy considerations

We do not anticipate any privacy ramifications from this proposal.

## Testing

We will add core tests that utilize the new syscall and verify all of the
behaviors documented above (page moves, constraints, and zeroing out of source
pages). We will also add a benchmark that measures the performance of this
syscall, which we can then compare to the performance of a copy.

## Documentation

We will add new docs describing `zx_vmo_transfer_data`.

## Drawbacks, alternatives, and unknowns

We could generalize `zx_pager_supply_pages` to work with anonymous VMOs. This
would add significant complexity to the implementation of that syscall, and
likely would still require an API modification to accept Anonymous VMOs as
input.

If our only goal is improving the performance of page lookup in CoW clone
hierarchies in bootfs, we could also just use copies of the VMO instead of
creating CoW clones. However, this significantly regresses boot times due to
the extra copy overhead.

<!--
// LINT.ThenChange(//tools/rfc/test_data/rfc.golden.md)
-->
