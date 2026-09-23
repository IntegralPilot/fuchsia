<!-- Generated with `fx rfc` -->
<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0254" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Problem Statement

The current behavior that Zircon exposes for attributing memory to VMOs is
proving problematic for the Starnix kernel in two ways:

First, Zircon's current behavior for memory attribution is only compatible with
the use of a widely-shared lock in Zircon's VMO implementation. This shared lock
is the cause of some performance problems when running many Starnix processes.

Starnix backs each `MAP_ANONYMOUS` Linux mapping with a VMO, and clones all such
VMOs in a Linux process when it makes a `fork()` call. With many Linux processes
the result is many VMOs contending on a small set of shared locks. When we
prototyped a memory-saving optimization where Starnix used one large VMO for all
mappings in a process, the shared lock degenerated to a "global" lock that all
Starnix VMOs contended on.

Second, Starnix can't emulate the exact behavior of Linux memory attribution
with the APIs Zircon provides. They don't distinguish between private and shared
copy-on-write pages in a VMO and hence also don't expose the share counts for
any such shared pages. Starnix needs this information to accurately compute
[USS] and [PSS] values that are exposed in various `/proc` filesystem entries.

We only propose changes to Zircon's memory attribution APIs and behavior that
will solve these problems while maintaining the existing feature set of tools
that rely on them. We don't seek here to solve the more general problem of
attributing memory between user space entities like components and runners. We
also don't intend these changes to be the "last word" for attribution - they are
a means to an end to solve the specific performance bottlenecks and feature gaps
Starnix faces today.

## Summary

Improve Zircon's memory attribution APIs and behavior to account for shared
copy-on-write pages.

## Stakeholders

_Facilitator:_

neelsa@google.com

_Reviewers:_

*   adanis@google.com
*   jamesr@google.com
*   etiennej@google.com
*   wez@google.com

_Consulted:_

*   maniscalco@google.com
*   davemoore@google.com

_Socialization:_

This RFC was socialized in conversations with stakeholders from the Zircon,
Starnix, and Memory teams. The stakeholders also reviewed a design document
outlining the changes proposed in this RFC.

## Requirements

1. The kernel's memory attribution implementation must not prevent optimizing
lock contention in Zircon by making the VMO hierarchy lock more fine-grained.
2. When tools like `memory_monitor` sum attribution for all VMOs in the system,
the total must agree with the system's actual physical memory usage. This is the
"sums to one" property.
3. Starnix must be able to emulate Linux memory attribution APIS on top of the
APIs provided by Zircon. This includes providing [USS], [RSS], and [PSS]
measurements and other measurements in relevant [proc filesystem] entries.

## Design

The changes we propose affect how Zircon attributes pages shared between VMO
clones via [copy-on-write]. These are COW-private and COW-shared pages.

We don't propose changes to how Zircon attributes pages shared between processes
via memory mappings or duplicate VMO handles. These are process-private and
process-shared pages.

User space is responsible for attributing process-shared pages to one or more
processes when appropriate. Zircon doesn't generally consider process sharing
in its decision-making, with one exception. See "Backwards Compatibility" for
more details.

The types of sharing are independent - a given page can be any combination of
COW-private, COW-shared, process-private, and process-shared.

### Zircon's Existing Behavior

Zircon's existing attribution behavior provide a single per-VMO measurement:
Attributed memory from the VMO's COW-private and COW-shared pages, with each
page unique attributed.

* It uniquely attributes COW-shared pages to the oldest living VMO which can
reference them. COW-shared pages are sometimes shared between sibling VMOs so
the oldest living VMO referencing a page need not be a parent of the others.
* Summing it over all VMOs system-wide measures the total system memory usage.
* It does not measure a VMO's [USS] (the memory which would be freed if that VMO
were destroyed), because it might include _some_ COW-shared pages.
* It also does not measure a VMO's [RSS] (the total memory the VMO references),
because it might not include _all_ COW-shared pages.
* It does not measure a VMO's [PSS] because that measurement scales each page's
contribution by the number of times it is shared.

### Zircon's New Behavior

Zircon's new attribution behavior provides three per-VMO measurements:

* [USS]: Attributed memory from the VMO's COW-private pages. This measurement
attributes the pages only to that VMO. It measures the memory which would be
freed if the VMO were destroyed.
* [RSS]: Attributed memory from the VMO's COW-private and COW-shared pages. This
measurement attributes the COW-shared pages to each VMO that shares them, so it
counts COW-shared pages multiple times. It measures the total memory the VMO
references.
* [PSS]: Attributed memory from the VMO's COW-private and COW-shared pages. This
measurement evenly divides attribution for each COW-shared page between all VMOs
which share that page. It can be a fractional number of bytes for a given VMO,
because each VMO gets an equal fraction of each of its shared pages' bytes. It
measures the VMOs "proportional" usage of memory. Summing it over all VMOs
system-wide measures the total system memory usage.

We chose to change the attribution APIs by exposing all of [USS], [RSS], and
[PSS] because it was the only option among the alternatives we explored that
satisfied all requirements.

### Impacts to User Space

This proposal continues to preserve the "sums to one" property which existing
user space tools rely on. Attribution queries may give different results under
the new behavior, but summing the queries for all VMOs in the system continues
to provide an accurate count of memory usage. These changes won't cause programs
like `memory_monitor` and `ps` to begin overcounting or undercounting memory,
and in fact will make their per-VMO and per-task counts more accurate.

The new APIs expose fractional bytes values for [PSS] and user space must opt-in
to observing these fractional bytes. The `_fractional_scaled` fields contain the
partial bytes and user space will slightly undercount [PSS] unless it uses them.

Starnix underestimates the [RSS] values for Linux mappings today, because the
current attribution behavior only counts COW-shared pages once. [RSS] should
count shared pages multiple times. Starnix can't emulate this behavior without
help from Zircon , so we change the Zircon attribution APIs to provide correct
[RSS] values. We could do this with another round of API changes, but to avoid
churn we chose to batch the changes together.

`memory_monitor` considers a VMO as shared with processes which reference any
any of its children, even for processes that don't reference the VMO directly.
It equally divides the parent VMO's attributed memory between these processes.
This attempts to account for COW-shared pages and allows `memory_monitor` to
"sum to one", but can lead to some incorrect results. For example, it can assign
a portion of a parent VMO's COW-private pages to processes which don't reference
that VMO and thus can't reference the pages. It also incorrectly scales parent
COW-shared pages when they are shared with some children but not others. Our
changes make this behavior redundant and we will remove it. The tool will still
scale the memory of VMOs when multiple processes reference them directly. See
"Backwards Compatibility" for more details.

### Syscall API Changes

Zircon's attribution APIs consist of `zx_object_get_info` topics which we will
change:

* `ZX_INFO_VMO`
  * We will preserve ABI and API backwards compatibility by:
      * Renaming `ZX_INFO_VMO` ->`ZX_INFO_VMO_V3` while retaining its value.
      * Renaming `zx_info_vmo_t` -> `zx_info_vmo_v3_t`.
  * We will add a new `ZX_INFO_VMO` and `zx_info_vmo_t`, and change the meaning
    of two existing fields in *all* versions of `zx_info_vmo_t`:

```cpp
typedef struct zx_info_vmo {
  // Existing and unchanged `zx_info_vmo_t` fields omitted for brevity.

  // These fields already exist but change meaning.
  //
  // These fields include both private and shared pages accessible by a VMO.
  // This is the RSS for the VMO.
  //
  // Prior versions of these fields assigned any copy-on-write pages shared by
  // multiple VMO clones to only one of the VMOs, making the fields inadequate
  // for computing RSS. If 2 VMO clones shared a single page then queries on
  // those VMOs would count either `zx_system_get_page_size()` or 0 bytes in
  // these fields depending on the VMO queried.
  //
  // These fields now include all copy-on-write pages accessible by a VMO. In
  // the above example queries on both VMO's count `zx_system_get_page_size()`
  // bytes in these fields. Queries on related VMO clones will count any shared
  // copy-on-write pages multiple times.
  //
  // In all other respects these fields are the same as before.
  uint64_t committed_bytes;
  uint64_t populated_bytes;

  // These are new fields.
  //
  // These fields include only private pages accessible by a VMO and not by any
  // related clones. This is the USS for the VMO.
  //
  // These fields are defined iff `committed_bytes` and `populated_bytes` are,
  // and they are the same re: the definition of committed vs. populated.
  uint64_t committed_private_bytes;
  uint64_t populated_private_bytes;

  // These are new fields.
  //
  // These fields include both private and shared copy-on-write page that a VMO
  // can access, with each shared page's contribution scaled by how many VMOs
  // can access that page. This is the PSS for the VMO.
  //
  // The PSS values may contain fractional bytes, which are included in the
  // "fractional_" fields. These fields are fixed-point counters with 63-bits
  // of precision, where 0x800... represents a full byte. Users may accumulate
  // these fractional bytes and count a full byte when the sum is 0x800... or
  // greater.
  //
  // These fields are defined iff `committed_bytes` and `populated_bytes` are,
  // and they are the same re: the definition of committed vs. populated.
  uint64_t committed_scaled_bytes;
  uint64_t populated_scaled_bytes;
  uint64_t committed_fractional_scaled_bytes;
  uint64_t populated_fractional_scaled_bytes;
} zx_info_vmo_t;
```

* `ZX_INFO_PROCESS_VMOS`
  * We will preserve ABI and API backwards compatibility by:
      * Renaming `ZX_INFO_PROCESS_VMOS` ->`ZX_INFO_PROCESS_VMOS_V3` while
        retaining its value.
  * We will add a new `ZX_INFO_PROCESS_VMOS`.
      * This topic re-uses the `zx_info_vmo_t` family of structs, so the changes
        to them all apply here.
* `ZX_INFO_PROCESS_MAPS`
  * We will preserve ABI backwards compatibility by:
      * Renaming `ZX_INFO_PROCESS_MAPS` ->`ZX_INFO_PROCESS_MAPS_V2` while
        retaining its value.
      * Renaming `zx_info_maps_t` -> `zx_info_maps_v2_t`.
      * Renaming `zx_info_maps_mapping_t` -> `zx_info_maps_mapping_v2_t`.
  * We will break API compatibility in one case:
      * Renaming `committed_pages` and `populated_pages` is a breaking change.
        See "Backwards Compatibility" below.
  * We will add a new `ZX_INFO_PROCESS_MAPS`, `zx_info_maps_t`, and
    `zx_info_maps_mapping_t`, and change the meaning of two fields in *existing*
    versions of `zx_info_maps_mapping_t`. These fields aren't present in the new
    version of this struct (see below):

```cpp
typedef struct zx_info_maps {
  // No changes, but is required to reference the new `zx_info_maps_mapping_t`.
} zx_info_maps_t;

typedef struct zx_info_maps_mapping {
  // Existing and unchanged `zx_info_maps_mapping_t` fields omitted for brevity.

  // These fields are present in older versions of `zx_info_maps_mapping_t` but
  // not this new version. In the older versions they change meaning.
  //
  // See `committed_bytes` and `populated_bytes` in the `zx_info_vmo_t` struct.
  // These fields change meaning in the same way.
  uint64_t committed_pages;
  uint64_t populated_pages;

  // These are new fields which replace `committed_pages` and `populated_pages`
  // in this new version of `zx_info_maps_mapping_t`.
  //
  // These fields are defined in the same way as the ones in `zx_info_vmo_t`.
  uint64_t committed_bytes;
  uint64_t populated_bytes;

  // These are new fields.
  //
  // These fields are defined in the same way as the ones in `zx_info_vmo_t`.
  uint64_t committed_private_bytes;
  uint64_t populated_private_bytes;
  uint64_t committed_scaled_bytes;
  uint64_t populated_scaled_bytes;
  uint64_t committed_fractional_scaled_bytes;
  uint64_t populated_fractional_scaled_bytes;
} zx_info_maps_mapping_t;
```

* `ZX_INFO_TASK_STATS`
  * We will preserve ABI and API backwards compatibility by:
      * Renaming `ZX_INFO_TASK_STATS` ->`ZX_INFO_TASK_STATS_V1` while retaining
        its value.
      * Renaming `zx_info_task_stats_t` -> `zx_info_task_stats_v1_t`.
  * We will add a new `ZX_INFO_TASK_STATS` and `zx_info_task_stats_t`, and
    change the meaning of three existing fields in *all* versions of the
    `zx_info_task_stats_t` struct:

```cpp
typedef struct zx_info_task_stats {
  // These fields already exist but change meaning.
  //
  // These fields include either private or shared pages accessible by mappings
  // in a task.
  // `mem_private_bytes` is the USS for the task.
  // `mem_private_bytes + mem_shared_bytes` is the RSS for the task.
  // `mem_private_bytes + mem_scaled_shared_bytes` is the PSS for the task.
  //
  // Prior versions of these fields only considered pages to be shared when they
  // were mapped into multiple address spaces. They could incorrectly attribute
  // shared copy-on-write pages as "private".
  //
  // They now consider pages to be shared if they are shared via either multiple
  // address space mappings or copy-on-write.
  //
  // `mem_private_bytes` contains only pages which are truly private - only one
  // VMO can access the pages and that VMO is mapped into one address space.
  //
  // `mem_shared_bytes` and `mem_scaled_shared_bytes` contain all shared pages
  // regardless of how they are shared.
  //
  // `mem_scaled_shared_bytes` scales the shared pages it encounters in two
  // steps: first each page is scaled by how many VMOs share that page via
  // copy-on-write, then each page is scaled by how many address spaces map the
  // VMO in the mapping currently being considered.
  //
  // For example, consider a single page shared between 2 VMOs P and C.
  //
  // If P is mapped into task p1 and C is mapped into tasks p2 and p3:
  // `mem_private_bytes` will be 0 for all 3 tasks.
  // `mem_shared_bytes` will be `zx_system_get_page_size()` for all 3 tasks.
  // `mem_scaled_shared_bytes` will be `zx_system_get_page_size() / 2` for p1
  // and `zx_system_get_page_size() / 4` for both p2 and p3.
  //
  // If P is mapped into task p1 and C is mapped into tasks p1 and p2:
  // `mem_private_bytes` will be 0 for both tasks.
  // `mem_shared_bytes` will be `2 * zx_system_get_page_size()` for p1 and
  // `zx_system_get_page_size()` for p2.
  // `mem_scaled_shared_bytes` will be `3 * zx_system_get_page_size() / 4` for
  // p1 and `zx_system_get_page_size() / 4` for p2.
  uint64_t mem_private_bytes;
  uint64_t mem_shared_bytes;
  uint64_t mem_scaled_shared_bytes;

  // This is a new field.
  //
  // This field is defined in the same way as the "_fractional" fields in
  // `zx_info_vmo_t`.
  uint64_t mem_fractional_scaled_shared_bytes;
} zx_info_task_stats_t;
```

## Implementation

We will implement the structural kernel API changes (e.g. adding and renaming
fields) as a single initial CL to reduce churn. We will set the value of all
new fields to 0 except for the `_fractional_scaled` fields which we will set
to a sentinel value of `UINT64_MAX`.

Next we will change the `memory_monitor` behavior both around attributing parent
VMO memory to processes and using the [PSS] `_scaled_bytes` fields instead of
the [RSS] `_bytes` fields. The sentinel values in the `_fractional_scaled`
fields will gate both of these backwards-incompatible behaviors for now. This
gating and the 0 values in the other new fields mean that no user space behavior
will change yet.

Then we will expose the new attribution behavior from the kernel. This change
will alter the meaning of existing fields such as `committed_bytes` and the new
fields can take on non-zero values. The `_fractional_scaled` fields cannot take
on the sentinel values any longer so user space will automatically begin using
the new behaviors and new fields to take advantage of the change.

Finally we will remove the checks on the `_fractional_scaled` fields from
`memory_monitor`.

The old versions of the APIs we change are considered deprecated. We can remove
these at a later date when we are confident no binaries reference them anymore.

## Performance

The new attribution implementation will improve performance for attribution
queries. It processes VMO clones fewer times and avoids expensive checks that
that were used to disambiguate shared COW pages between multiple VMOs.

Later, implementing the fine-grained hierarchy lock will improve performance for
page faulting and several VMO syscalls including  `zx_vmo_read`, `zx_vmo_write`,
`zx_vmo_op_range`, and `zx_vmo_transfer_data`. Today these operations contend on
a hierarchy lock shared by all related VMOs, which serializes all of these
operations on related VMO clones. The serialization is much worse for VMOs that
are cloned many times. Many Starnix and bootfs VMOs fall into this category.

Zircon will perform more work when creating and destroying child VMOs. Creating
`SNAPSHOT` and `SNAPSHOT_MODIFIED` children becomes `O(#parent_pages)` instead
of `O(1)`. Destroying any type of child becomes `O(#parent_pages)` in all cases
where before it could be `O(#child_pages)` in some cases. Zircon currently
defers this work to more frequent operations such as page faults. Moving it to
the infrequent `zx_vmo_create_child()` and `zx_vmo_destroy()` operations will
make those other frequent operations faster in exhcnage. We don't expect any
user-visible performance regressions as a result.

We will use existing microbenchmarks and macrobenchmarks to verify the expected
performance deltas.

## Backwards Compatibility

### ABI and API Compatibility
We will create additional versioned topics for all kernel API changes and
preserve support for all existing topics. This will preserve ABI compatibility
in all cases and API compatibility in all cases except one.

Renaming `committed_pages` and `populated_pages` in `zx_info_maps_mapping_t`
is an API-breaking change. These fields are only used in Fuchsia-internal
tools in a handful of places, so the CL which renames these fields will also
change the names at all call sites.

Other changes won't require updating existing callsites unless they want to take
advantage of newly-added fields.

### Behavioral Compatibility

Starnix, `ps`, and a few libraries external to Fuchsia compute per-VMO or
per-task RSS values. The new attribution behavior will make these computations
more accurate, because they will count COW-shared pages multiple times. This
will change the RSS values in a backwards-incompatible way, but only by making
them more accurate so we don't expect any negative effects.

`memory_monitor`'s method of attributing memory for parent VMOs has correctness
issues which the new attribution behavior can fix. We must change this tool's
implementation in a backwards-incompatible way to implement the fix. To avoid
churn we will set the new `_fractional_scaled` fields to a sentinel value of
`UINT64_MAX` until we implement the new attribution behavior. We will gate
`memory_monitor`'s implementation on this value. Attribution never generates
this sentinel value normally, so it makes a great "feature flag".

We preserve the meaning of "shared" as "process-shared" in `ZX_INFO_TASK_STATS`
despite it functioning differently from the other attribution APIs. This allows
existing programs which depend on the `ZX_INFO_TASK_STATS` behavior to continue
functioning without invasive changes. Future work might replace these uses of
`ZX_INFO_TASK_STATS` with different queries, perhaps to `memory_monitor`, at
which time we could deprecate and remove `ZX_INFO_TASK_STATS`.

## Testing

The core test suite already has many tests which validate memory attribution.

We will add a few new tests to validate edge cases around the new attribution
APIs and behavior.

## Documentation

We will update syscall documentation for `zx_object_get_info` to reflect these
changes.

We will add a new page under `docs/concepts/memory` which describes Zircon's
memory attribution APIs and provides examples.

## Drawbacks, alternatives, and unknowns

We explored some alterantive solutions models in the process of developing this
proposal. Most were easy to implement but had undesriable performance or
maintainability characteristics.

### Emulate Current Attribution Behavior
In this option we would change the kernel to emulate the existing attribution
behavior on top of a new implementation.

It has no backwards-compatibility concerns and doesn't require any API changes.

However it blocks the use of fine-grained hierarchy locking because it needs
the existing shared hierarchy lock to function. The emulation would cause
attribution queries to deadlock under a finer-grained locking strategy. It would
also make attribution queries perform more badly; their performance is already
of concern today. Finally, it doesn't provide the information Starnix needs to
compute [USS], [RSS], or [PSS] measurements.

### Expose Only USS and RSS
In this option we would change the kernel to attribute shared copy-on-write
pages to each VMO sharing them, and expose attribution for private and shared
pages separately.

It is compatible with a fine-grained hierarchy lock.

However it doesn't provide user space with enough information to preserve the
"sums to one" property or for Starnix to compute [PSS] measurements. The kernel
attributes COW-shared pages multiple times with this option. Each COW-shared
page can be shared a different number of times depending on write patterns, so
user space would require per-page information to deduplicate COW-shared pages or
evenly divide them amongst clones.

### Expose Detailed per-VMO-tree Information
In this option we would change the kernel to attribute shared copy-on-write
pages to each VMO sharing them, and expose per-tree and per-VMO information:

* A tree identifier which links a VMO to its tree
* A stable per-VMO identifier for tie-breaking, like a timestamp
* Private pages per VMO
* Total shared pages per tree
* Number of per-tree shared pages visible by each VMO

It provides user space with the information needed to satisfy the "sums to one"
property.

However it blocks the lock contention optimizations we wish to make because it
needs the existing shared hierarchy lock to function. It isn't feasible to
compute the "total shared pages per tree" without such a shared lock. This
option also exposes kernel implementation details to user space that would make
future maintenance of VM code more difficult. It also doesn't provide the
information Starnix needs to compute [PSS] measurements.

## Prior art and references

Other operating systems provide the [RSS], [USS], and [PSS] measurements used in
this RFC, although sometimes under different names. Windows, MacOS, and Linux
all track [RSS] and [USS]. Linux-based operating systems track [PSS].

Linux exposes these measurements via the [proc filesystem]. Attribution tools
like ps and top exist to present them in a user-friendly way.

<!-- xrefs -->
[RSS]: https://en.wikipedia.org/wiki/Resident_set_size
[USS]: https://en.wikipedia.org/wiki/Unique_set_size
[PSS]: https://en.wikipedia.org/wiki/Proportional_set_size
[copy-on-write]: https://en.wikipedia.org/wiki/Copy-on-write
[proc filesystem]: https://man7.org/linux/man-pages/man5/proc.5.html
