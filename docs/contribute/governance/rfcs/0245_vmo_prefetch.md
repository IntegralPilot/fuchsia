<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0245" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Summary

Have a read-only analogue of `ZX_VMO_OP_COMMIT` that populates a VMO for
imminent read accesses.

## Motivation

The existing COMMIT operations, `ZX_VMO_OP_COMMIT` and `ZX_VMAR_OP_COMMIT`, are
a performance tool for users. They allow the user to indicate that ranges of a
VMO or VMAR are going to be used and allow the kernel to more efficiently bulk
allocate all the pages, avoiding the need to take many faults in the future.

COMMIT has two downsides today stemming from its intent of simulating a write.
First, it requires the WRITE permissions on the VMAR and/or VMO, which may not
be had if this is for some executable data. Second, it actively performs
copy-on-write and allocates pages, which is not necessary and a waste of memory
if the range is only going to be read from in the future.

Today, Starnix is impacted by the first downside stemming from its use of
read-only pager backed VMOs. A PREFETCH operation would resolve both of the
downsides, while retaining the desired benefits of COMMIT.

## Stakeholders

_Facilitator:_

cpu@google.com

_Reviewers:_

adamperry@google.com, dworsham@google.com

_Consulted:_

rashaeqbal@google.com

_Socialization:_

This feature came about via discussion between the Starnix and virtual memory
teams.

## Requirements

A PREFETCH operation should be added to VMOs, as `ZX_VMO_OP_PREFETCH` and VMARs,
as `ZX_VMAR_OP_PREFETCH`. These operations should only require the READ
permission on their respective handles and should do the work necessary such
that future read operations have minimal work to do. PREFETCH may therefore:

 * Requesting any missing pages from a user pager in the range
 * Decompress any pages in the range
 * For VMARs, create hardware page tables for any pages in the range

## Design and implementation

The kernel VM internals already have most of the tools to support this feature,
and implementation is largely plumbing the new API flags. As such this can be
implemented in one, or two if the VMO and VMAR implementations are split,
small CLs, including any relevant testing and documentation etc.

## Performance

This change should not impact the performance of any existing functionality, to
be verified with our existing benchmarks.

## Security considerations

PREFETCH is logically equivalent to the user performing manual reads across the
range. Other than needing the READ permission instead of the WRITE permission,
the rest of the requirements and restrictions are exactly the same as for
COMMIT, and so there are no new security considerations here.

## Testing

Tests will be added to the core test suite to validate that the PREFETCH
operations are supported and trigger user pager requests correctly.

Testing other aspects from user space, such as PREFETCH triggering
decompressions, is difficult due to these systems intentionally being largely
transparent to the user, modulo performance. As performance based unit tests are
inherently flaky, especially due to emulator execution, these behaviors will be
tested as far as possible with kernel unit tests.

## Alternatives

### Change COMMIT to simulate read

Instead of introducing a PREFETCH operation, the existing COMMIT operations
could have their semantics changed to instead simulate reads. Although this
would be sufficient for the Starnix use case, other existing users do want the
write simulation and to have pages eagerly allocated in order to avoid faults
and allocations in future writes.

### Keep using ALWAYS_NEED

Starnix presently works around the lack of a PREFETCH operation by using the
ALWAYS_NEED operation. ALWAYS_NEED performs the same initial action as PREFETCH
needs to, that of bringing in any missing pages and decompressing etc. Unlike
PREFETCH, it makes an additional statement that this memory should not be
reclaimed if at all possible. This contributes to system memory pressure and may
increase the likelihood of OOMs or degraded user experience.

### Additional zx_vmar_map options

In addition to supporting a VMAR operation, PREFETCH could be supported at
mapping creation time via an additional option such as
`ZX_VM_MAP_RANGE_PREFETCH`.
This would be an optimization to avoid needing to issue the PREFETCH operation
after creating the mapping.

It is presently unclear if avoiding the extra syscall is necessary and this RFC
does not wish to rule out additional support in `zx_vmar_map`, but it needs
separate motivation that is out of scope and can be done in a follow up proposal
when and if needed.
