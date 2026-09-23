<!-- Generated with `fx rfc` -->
<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0236" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Summary

The objective is to introduce a new VMO child type that allows a snapshot to
be taken of any modified pages in the child of a pager-backed VMO.


## Motivation

The kernel today supports two types of VMO clones (also called child VMOs):
true snapshots in which neither VMO sees each other's changes after the clone
operation completes, and snapshot-at-least-on-write clones, in which the child
can see the changes on the parent VMO, after the operation completes, except
for the pages on the child that have been written to.

The cloning operation can be repeated on the VMO clones themselves, creating a
hierarchy of child VMOs. For ease of implementation the Zircon kernel does not
allow mixed hierarchies; you can either create a hierarchy of true snapshots or
a hierarchy of snapshot-on-write VMOs.

With the invention of Starnix, fork() must be efficiently supported by Fuchsia.
Fork requires cloning the entire address space of the parent process into the
child process, which includes anonymous memory but also pager-backed data and
code VMOs.

A problem arises because the kernel only supports true snapshots for anonymous
memory, while for pager-backed VMO it only supports snapshot-on-write clones.
Therefore the fork() contract cannot be met with cloning: so Starnix is forced
to implement eager copies and/or other cpu and memory intensive workarounds.

## Stakeholders

Who has a stake in whether this RFC is accepted? (This section is optional but
encouraged.)

csuter@google.com, jamesr@google.com

_Facilitator:_

davemoore@google.com

_Reviewers:_

rashaeqbal@google.com, jamesr@google.com

_Consulted:_

List people who should review the RFC, but whose approval is not required.

csuter@google.com, adanis@google.com, cpu@google.com, mvanotti@google.com,
lindkvist@google.com


_Socialization:_

A design document was socialized with the Zircon team and some members of
starnix and a work in progress CL was shared with stakeholders for benchmarking.

## Requirements

- Efficient support for cloning address spaces that have both pager-backed VMOs
and anonymous VMOs.
- There should not be wasted memory after a VMO in inaccessible by a process
(last handle is closed) for either parent VMOs or child VMOs, in other words,
these two cases:

```
vmo = create_vmo();
   loop {
      child = create_snapshot_modified(vmo)
      child.write(...)
      vmo = child  // old vmo is dropped
   }
```

```
vmo = create_vmo();
   loop {
      vmo.write(...)
      child = create_snapshot_modified(vmo)
      // child is dropped
   }
```

## Design

`ZX_VMO_CHILD_SNAPSHOT_MODIFIED` is a new type of VMO clone that will allow the
 creation of a snapshot-modified child type.

This flag creates a child that retains a snapshot of any pages that have been
modified by a child of a pager-backed VMO. Semantically, it is as though eager
copy is performed on any pages in the parent not backed by a pager. Pages in
the parent that are backed by a pager will behave with at least copy-on-write
semantics into the clone. This differs from the original snapshot semantics
which behave as though an eager copy is created on all pages in the VMO.

When used for the first time on a pager-backed VMO, the semantics behave as
though `SNAPSHOT_AT_LEAST_ON_WRITE` were used. A handle to a clone is created
which is initially identical to the parent, but modifications can be made on
the clone which will cause it to diverge.

When used against any anonymous VMO, a `SNAPSHOT_MODIFIED` clone will be
upgraded to have snapshot semantics, similar to the existing clone type upgrade
semantics that are used by `SNAPSHOT_AT_LEAST_ON_WRITE`.

This flag is not valid for cloning VMOs with pinned regions, slices or VMOs
descended from  [`zx_vmo_create_physical()`] or [`zx_vmo_create_contiguous()`].

### Cases

#### Snapshot-modified on pager-backed VMO

Creating a single `SNAPSHOT_MODIFIED` clone of a pager-backed VMO will behave
as though a single `SNAPSHOT_AT_LEAST_ON_WRITE` clone was created. At the time
the clone was performed, the new VMO will be identical to the parent.

Until another `SNAPSHOT_MODIFIED` is performed on the clone, it can still be
modified. Any unmodified pages in the clone will have at least copy-on-write
semantics.

#### Snapshot-modified after at-least-on-write or snapshot-modified
#### of a pager-backed VMO

As both `SNAPSHOT_MODIFIED` and `SNAPSHOT_AT_LEAST_ON_WRITE` behave the same
way on a pager-backed VMO, both cases of calling `SNAPSHOT_MODIFIED` on a clone
of the pager-backed will result in the same semantics. Any pages that are
no longer pager-backed will have a snapshot, and pager-backed pages will have at
lease copy-on-write semantics.

#### Snapshot-modified after snapshot

In this case the semantics will be upgraded to snapshot, similar to
snapshot-at-least-on-write.

### Unsupported Cases

The following cases are currently unsupported, and if a `SNAPSHOT_MODIFIED`
clone is attempted, `ZX_ERR_NOT_SUPPORTED` will be returned.

#### Snapshot-modified end of snapshot-at-least-on-write chain

Snapshot-modified could potentially be expanded to be used at the end of a
snapshot-at-least-on-write chain. This could have confusing results however as
un-forked pages from the cloned VMO can see modifications all the way through
the unidirectional VMO chain, with the closest relative being the one that is
read. This creates inconsistencies with the original promise stating that
snapshots can be created of any modified page.

#### Snapshot-modified the middle of a snapshot-at-least-on-write chain

Snapshot-modified could never be used on a VMO that has children (i.e. in the
middle of a snapshot-at-least-on-write chain) as it can create inconsistent
hierarchies.


### Nomenclature

The existing naming convention for clone type flags in [`zx_vmo_create_child()`]
aims to name flags in a way that describes the provided semantics. The current
name for this flag is `SNAPSHOT_MODIFIED` as it summarizes the behavior in
which the modified pages in the clone are snapshot. A similar option is
`SNAPSHOT_MODIFICATIONS`. Some other considered flags were `SNAPSHOT_MIXED`
which does describe the semantics, but is less clear
`SNAPSHOT_PAGER_MODIFICATIONS` was another consideration, but it isn't ideal
to couple the VMO with the pager.


## Implementation

Snapshot-modified affects a number of files in Zircon, but can be broken into
CLs that add support for the new snapshot type in kernel internals before the
option flag is added to the syscall.

Some in-kernel testing will be added to validate correct behavior of the new
structure during the first stage, and more complex core tests will be included
with the introduction of the option flag.

## Performance

In most cases the newly added code will only be called on the creation of a new
snapshot-modified clone, so performance of existing code is unexpected to
change. The one exception is that when creating a `SNAPSHOT_AT_LEAST_ON_WRITE`
child the naive approach includes an additional acquisition of the VMO lock, but
if this introduces a performance penalty it will be trivial to refactor the
clone selection code to avoid this.

## Security considerations

Snapshot-modified is unlikely to introduce any vulnerabilities as it is build
using existing Zircon primitives and no new functionality is introduced.

## Testing

Kernel unit tests and core tests will be included with the relevant CLs.

## Documentation

A detailed design document aimed at Zircon developers will be released. It
will outline the new data structures, supported and unsupported cases, changes
to the code, challenges and alternatives.

A new flag will be added into the [`zx_vmo_create_child()`] with the
description:

`ZX_VMO_CHILD_SNAPSHOT_MODIFIED` - Create a child that behaves as though an
eager copy was performed on any pages in the parent not backed by a pager, i.e.
pages that have been modified by a child of a pager-backed VMO. Pager-backed
pages will have at least copy-on-write semantics. This flag may not be used for
VMOs created with zx_vmo_create_physical(), zx_vmo_create_contiguous(), VMOs
containing pinned pages, or descendants of such VMOs. This flag is also not
supported for any VMOs created with `SNAPSHOT_AT_LEAST_ON_WRITE` that have
non-slice children or are not the child of a pager-backed VMO.

## Drawbacks, alternatives, and unknowns

Creating snapshot semantics for a pager backed VMO is non-trivial using
existing Zircon VMO primitives. The user pager operates by servicing page
requests to a single VMO, and at present its children form a single,
unidirectional chain with copy-on-write semantics.

The snapshot flag that can be used on an anonymous VMO creates a hidden
VmCowPages that is a common ancestor to the target VMO and its new snapshot. As
there is nothing pointing to the hidden VmCowPages that can modify it's
pages, it is immutable. This hidden VmCowPages retains the pages from the
target VMO, modifications in the children have copy-on-write semantics.
Therefore, a search for pages in this hierarchy involves a walk up the tree
that can end at the hidden root.

It would be difficult to use the existing snapshot data structure for use by
the pager, as the root VMO would always become hidden, with the original VMO
becoming the left child. If the pager remains pointing to the original VMO
(which now has a hidden parent), pager operations would have to be propagated
up to the hidden root, to supply it pages at the request of its child. This
creates inconsistencies as pages are added to a node that is not being operated
on.

The simplest solution was to create a mixed hierarchy, where the root is
visible & has a single hidden child that acts as the hidden root to a snapshot
tree.

There is more than one way to describe the provided semantics to users. The
description in the RFC outlines the provided behavior with respect to the
pager, but an alternative way of framing it is on the modification & cloning of
pages only. An example of this would be:

"This flag will create a child that retains a snapshot of any modified pages
from the parent. If the root vmo writes to an unmodified page after the
snapshot has occurred, the snapshot-modified child will see the changes. This
differs from the original snapshot semantics which behave as though an eager
copy was created."

This descripion is correct, but it requires additional clarification of the
caveat that a pager is required for this behavior, as the flag will upgrade to
snapshot semantics when used on an anonymous VMO.

### Could SNAPSHOT_MODIFIED replace SNAPSHOT_AT_LEAST_ON_WRITE?

It would be non-trivial to phase out snapshot-at-least-on-write and replace it
with snapshot-modified as the semantics of the two clone types differ, which
could cause unexpected behavior. Although both clone types offer 'at least
copy-on-write' semantics, snapshot-modified can have a mix of snapshot and
at-least-on-write pages within the same VMO. Additionally, the change would
require performance testing. When a VMO is pager-backed,
snapshot-at-least-on-write allocates less memory per clone created as there are
no hidden, common ancestors created. Thus, migrating all uses of
snapshot-at-least-on-write could introduce performance regressions in some use
cases.

It could be worth investigating replacing `SNAPSHOT_AT_LEAST_ON_WRITE`,
however, as it would simplify the API for [`zx_vmo_create_child()`] as most
fdio helpers promise semantics that are compatible with `SNAPSHOT_MODIFIED`.



## Prior art and references

[`zx_vmo_create_child()`]



[`zx_vmo_create_child()`]: /reference/syscalls/vmo_create_child.md
[`zx_vmo_create_contiguous()`]: /reference/syscalls/vmo_create_contiguous.md
[`zx_vmo_create_physical()`]: /reference/syscalls/vmo_create_physical.md
