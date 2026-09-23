<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0219" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Summary

This document proposes adding an in-kernel compression and decompression system
for anonymous user memory.

Although the system is in-kernel, it is not completely transparent to user-space
due to impacts to memory accounting. But aside from API changes, all functional
changes are contained to the kernel.

## Motivation

Fuchsia is frequently used on resource constrained devices where having an
increased memory pool would greatly improve user experiences.

Kernel compression of memory is a standard technique used by other operating
systems and supporting it would help Fuchsia gain parity.

## Stakeholders

_Facilitator:_

cpu@google.com

_Reviewers:_

eieio@google.com, rashaeqbal@google.com, maniscalco@google.com

_Consulted:_

mseaborn@google.com, mvanotti@google.com, wez@google.com, plabatut@google.com,
jamesr@google.com, davemoore@google.com

_Socialization:_

Pre-RFC proposal doc of kernel design and implementation parts shared to Zircon
Kernel team and wider stakeholders.

## Design

This section outlines the high level design choices of the implementation,
compression strategies and proposed API changes and extensions.

### Aging and tracking

Anonymous pages should be treated similarly to pager backed pages for the
purposes of aging, and so that implies:

 * Adding backlinks to anonymous pages so that `vm_page_t` to `VmObjectPaged` +
   offset lookups can be performed. This adds a cost of needing to update
   backlinks whenever pages move around VMOs.
 * Changing the page queues to not treat pager backed and anonymous pages
   differently.

Aging in the page queues has an existing notion of an active and inactive set.
Existing eviction will never evict from the active set to prevent thrashing
conditions. Anonymous pages will be part of the same queues and have the same
restrictions for compression.

Pages, if compressed, are removed from the page queue tracking, similar to
evicted pages. Pages that fail compression will be moved to a separate list so
that compression is not attempted again. Should such a page get accessed it will
move back to the active set, and potential age and have compression attempted
again.

Although we will only compress inactive pages, there are three triggers we will
support:

 * Eager compression of old pages, even if not under memory pressure, when
   processing the LRU queue.
 * Compression under memory pressure, in injunction with performing eviction.
 * Compression to avoid OOM.

These triggers will be toggled via kernel cmdline flags.

### VmPageList markers

The `VmPageList` stores needs to have an additional kind of marker that can:

 * Indicate that a page exists, but is compressed.
 * Store sufficient meta-data to find the page in the compressed storage.

Entries of the `VmPageList` are already encoded to store both `vm_page_t`
pointers and zero page markers, and can be extended to have an additional
encoding.

Currently a page list entry is a 64-bit value, which is encoded into the
following options:

 * 0: Empty entry
 * Least significant bit = 1: Zero page marker
 * Anything else: pointer to a `vm_page_t`

This scheme works due to pointer alignment making bottom 3 bits of a `vm_page_t`
pointer always zero. Therefore we can generalize this scheme such that the
bottom 3 bits of the page list entry represent the type, and the remaining bits
are associated data. Initial types would be:

 * 0: Empty entry or `vm_page_t` pointer (depending if the data bits are set)
 * 1: Zero page marker. Remaining data bits required to be 0.
 * 2: Compressed page. Remaining bits are an opaque token returned from the
      compression system.
 * 3-7: Invalid types for the moment.

### Compression operation

To focus on the simple and efficient initial use cases the initial storage
strategies will perform single page compression and decompression, with no
explicit grouping of pages by VMO.

Not grouping pages by VMO allows the compression path to be fed in the same way
as the existing eviction system. Although pages from different VMOs will
effectively be jumbled up in a single storage system, it is a strict requirement
that lock dependencies not be created between different VMOs. This is to both
prevent information leaks via timing channels, and to prevent penalizing
unrelated processes with high latency.

To avoid unnecessary lock contention, the execution of either the compression or
decompression algorithms should happen without holding the VMO lock, and
compression should not happen while holding any locks needed for decompression.

#### Compression algorithm

What follows is a high level description of the compression path, with error
handling cases elided. The input to compression is a page, the VMO it is owned
by and the offset of that page in the VMO. The offset is used to validate that
the page is still in the VMO once the VMO lock is acquired, which falls under
error handling that is otherwise skipped in this pseudo-code.

```
Take VMO lock
Update page list to use temporary compression reference
Release VMO lock
Compress page to buffer
Take compressed storage lock
Allocate storage slot
Release storage lock
Copy buffer to storage
Take VMO lock
Check page list still holds temporary compression reference
Update page list to final compression reference
Release VMO lock
```

The motivation to use a temporary reference initially, instead of immediately
allocating a reference, is to provide greater flexibility to the compressed
storage system in generating references by giving it the size of the data it
will be storing when generating the reference. This can allow it to represent
the exact location of the data in the reference, without needing to indirect
through an additional table.

Even if a temporary reference were not used, two acquisitions of the VMO lock
would still be needed in order to resolve any potential races to use the page
while compression was happening. For example, the page (in this case reference)
could have been decommitted while compression was happening, and we would not
know this unless we reacquired the VMO lock again and checked.

While compression is happening, since the VMO lock is not held, another thread
may find the temporary reference and attempt to use it. As the temporary
reference does not actually have data that can be decompressed we have two
options to resolve this:

 * Wait for compression to complete, then retrieve the original page.
 * Make a copy of the original page (the memory for this would be pre-allocated
   at the start of a compression run).

The purpose of this is to ensure that the page is owned and in a known state the
entire time compression is happening. Using the original page while compression
is ongoing is problematic since:

 * Modifying the page while the compression algorithm is reading it increases
   the potential bugs and attack surface of the compression algorithm as it may
   not be resilient to concurrent data changes.
 * The VMO could have its cacheability or other properties changed, making the
   parallel cached usage by the compressor undefined behavior.

The proposal here is to copy the original page, since this should be a very
unlikely scenario and copying will both resolve the request faster and avoid
needing to have another synchronization mechanism. Here the copy is happening
under the VMO lock, however this is consistent with all other copy-on-write
paths where a page copy can happen under the VMO lock. As there will only be a
single compression happening at a time, any given VMO operation can hit this
scenario at most once.

#### Decompression algorithm

Decompression is implemented to leverage the existing `PageRequest` system,
which is already used for delayed allocations and fulfilling user pager
requests. In a similar way that an absent page for a user pager VMO generates a
page request, a compressed reference in the VMO's page list would trigger a
similar process:

```
Take VMO lock
Observe page is compressed
Fill in the PageRequest
Drop VMO lock
Wait on the PageRequest
Retry operation
```

When filling in the page request we need to have stored the compressed reference
from the page list, along with the VMO and offset. This will require augmenting
the existing `PageRequest` structure.

The difference from a user pager page request and compression is that the act of
waiting can directly resolve the request, which in this case means performing
decompression.

Decompression needs to be tolerant of multiple threads attempting to access the
same page and provide a way for priority inheritance to flow:

```
Declare stack allocated OwnedWaitQueue
Take compressed storage lock
Validate compressed reference
if compressed page has wait queue then
    take thread_lock
    release compressed storage lock
    wait on referenced wait queue
else
    store reference to our allocated wait queue in compressed block
    release compressed storage lock
    decompress into new page
    take VMO lock
    update VMO book keeping
    release VMO lock
    take compressed storage lock
    take thread lock
    remove wait queue reference
    release compressed storage lock
    wake all threads on wait queue
```

Here the `thread_lock` is used only because that is the lock required to be held
for performing wait queue operations, and no other properties of holding the
`thread_lock` are being leveraged.

#### Target compressed size

Due to the cost associated with decompressing, it is only worth storing the
compressed version of a page if it has a sufficient reduction in size. Otherwise
the uncompressed version might as well be stored, saving the decompression cost.

This target size should be a tunable option, although targets of 70% are common
in other system.

Any compression attempt that would not fit within this target is considered to
have failed, and the page is placed back in the page queues as described in the
aging and tracking section.

#### Zero page scanning

As compression fundamentally requires inspecting all the bytes in the source
page, it presents an opportunity to detect and de-duplicate zero pages. This
represents an alternate success result of compression, where it reports that the
input page is just the zero page, and does not store the result in the
compressed storage system. Instead the VMO caller can replace the page with a
zero page marker.

Off the shelf compression algorithm implementations do not typically have a way
to report that the input was all zeroes, and so realizing this requires either:

 * Modifying or creating an implementation to perform this tracking.
 * Comparing the compressed result with a known signature.

The second option leverages the fact that compression algorithms are
deterministic, and typically have zero page representations in the low tens of
bytes. Performing a size check followed by a memory comparison check may
therefore be sufficiently cheap.

This proposal does not require an initial compression implementation to perform
zero page detection, but the APIs and VMO logic must be built to support it.
Eventually the existing zero page scanner should be replaced in favor of this
being supported by compression.

### Compression algorithm

LZ4 is proposed as the initial compression algorithm. See the experimental
evaluation section for more details, but it has the important requirements of:

 * Works reasonably well for small sizes (4KiB).
 * Does not perform `malloc` or equivalents during its steady post init
   operation.
 * Can decompress in parallel with no shared mutable state. Compression must be
   able to happen in parallel to decompression.
 * Although not required, multiple parallel compression itself is supported.

LZ4 also has the additional useful property of being able to abort compression
early based on restricted target buffer size.

### Compressed storage

Storage of the compressed pages is a trade off between code complexity and run
time with potential fragmentation and increased memory usage. An initial
strategy of packing a fixed number of compressed pages, in this case three, into
single storage pages is chosen.

The core concept of this storage strategy is that each storage page can be
considered to have a left, middle and right storage slot. which allows at best a
3:1 compression ratio.

Implementation is quite straight forward and involves:

 * Search any existing partially filled pages for a slot.
 * If no existing suitable slot, place in the left slot of a new page.

The two complications are:

 * Partially filled pages should be bucketed into search lists based on the size
   of their free slot for nearly optimal packing with O(1) search.
 * The middle slot can become unavailable if the left or right slot has large
   enough data in it.

Benefits of this kind of strategy are:

 * Simple implementation.
 * Predictable operation with no long tail performance.
 * Degrades in the worst case to not using any additional memory.

The one pathological behavior that might need to be handled is once pages have
been uncompressed there might exist holes that could be filled and enable other
storage pages to be freed. Such a compaction system can be implemented if and
when it is needed.

Longer term this could be evolved into being, or using, a more generic heap or
slab style allocator.

See the experimental evaluation section for some initial results.

### Accounting and Metrics

Although the operation of compression happens without any user input it does
effect how memory usage should be reported. We would also like to exfiltrate
additional information to user space on how compression is performing.

#### Content vs committed

Existing querying interfaces refer to committed bytes and pages to mean actual
memory allocated directly in the VMO to hold data.

 These existing queries are:

 * `ZX_INFO_TASK_STATS` that reports committed memory in `mem_private_bytes`,
   `mem_shared_bytes`, etc.
 * `ZX_INFO_PROCESS_MAPS` where `committed_pages` of the mapped VMO are
   reported.
 * `ZX_INFO_PROCESS_VMOS` / `ZX_INFO_VMO` that reports a `committed_bytes`.
 * `ZX_INFO_KMEM_STATS` / `ZX_INFO_KMEM_STATS_EXTENDED` that reports `vmo_bytes`
   that are counted as committed.

We will add an additional notion of content bytes/pages in addition to
committed. Any queries that talks about committed memory continue to apply to
mean specifically memory that directly holds content in the VMO. Content will
instead mean the amount of data that the kernel is holding for the user, without
being specific as to how. That is, content could be held directly in pages, and
therefore also be counted in committed, or be compressed and not be counted in
committed.

Content that is compressed is, at this scale, considered to be of an unknown
storage size. Attempting to attribute individual compressed pages in storage
would be complicated and unstable, since storage size would depend on total
system compression behavior and the resulting fragmentation.

Although anonymous VMOs start as being notionally zero, this is not considered
content that the kernel is remembering. If the user explicitly commits or
modifies any pages, that it becomes content. After this, even if the user leaves
or resets the content to zero, the kernel is not obligated to notice this and
may continue considering it as content that it is tracking.

In relation to eviction and user pagers, content that can be retrieved from a
user pager is not considered content tracked by the kernel. Eviction, therefore,
subtracts from both committed and content amounts.

#### Metrics

To understand how compression is performing, both for tuning / development and
for continuous system health monitoring, we want to collect and provide metrics.

These metrics should be able to answer questions on how compression is
performing, as well as how it is impacting system performance. Specifically we
would like to know things like:

 * Compression storage ratio.
 * How long pages are spending compressed.
 * What kinds of pages are being compressed and what is triggering it.
 * Decompression latency.
 * CPU time spent performing compression and decompression.
 * Ratio of successful versus failed compression attempts.

#### API Changes

Extend, via struct evolution and query versioning:

 * `zx_info_maps_mapping_t` to have `content_pages` field.
 * `zx_info_vmo_t` to have `content_bytes` field.
 * `ZX_INFO_TASK_RUNTIME` to report `page_fault_decompress_time` as a subset of
   total `page_fault` time.

The `zx_object_get_info` queries that need versioning for these are:

 * `ZX_INFO_PROCESS_MAPS`
 * `ZX_INFO_PROCESS_VMOS`
 * `ZX_INFO_VMO`
 * `ZX_INFO_TASK_RUNTIME`

Existing fields such as `committed_bytes` from `zx_info_vmo_t` will have their
comments and documentation updated to clarify that they do not count pages that
are in the compressed state.

Exact contents are TBD as part of the iteration phase, but propose to add a
`ZX_INFO_KMEM_STATS_COMPRESSION` query to report:

```c
struct zx_info_kmem_stats_compression {
    // Size in bytes of the content that is current being compressed.
    uint64_t uncompressed_content_bytes;

    // Size in bytes of all memory, including metadata, fragmentation and other
    // overheads, of the compressed memory area. Note that due to base book
    // keeping overhead this could be non-zero, even when
    // |uncompressed_content_bytes| is zero.
    uint64_t compressed_storage_bytes;

    // Size in bytes of any fragmentation in the compressed memory area.
    uint64_t compressed_fragmentation_bytes;

    // Total amount of time compression has spent on a CPU across all threads.
    // Compression may happen in parallel and so this can increase faster than
    // wall clock time.
    zx_duration_t compression_time;

    // Total amount of time decompression has spent on a CPU across all threads.
    // Decompression may happen in parallel and so this can increase faster than
    // wall clock time.
    zx_duration_t decompression_time;

    // Total number of times compression has been done on a page, regardless of
    // whether the compressed result was ultimately retained.
    uint64_t total_page_compression_attempts;

    // How many of the total compression attempts were considered failed and
    // were not stored. An example reason for failure would be a page not being
    // compressed sufficiently to be considered worth storing.
    uint64_t failed_page_compression_attempts;

    // Number of times pages have been decompressed.
    uint64_t total_page_decompressions;

    // Number of times a page was removed from storage without needing to be
    // decompressed. An example that would cause this is a VMO being destroyed.
    uint64_t compressed_page_evictions;

    // How many pages compressed due to the page being inactive, but without
    // there being memory pressure.
    uint64_t eager_page_compressions;

    // How many pages compressed due to general memory pressure.
    uint64_t memory_pressure_page_compressions;

    // How many pages compressed due to attempting to avoid OOM or near OOM
    // scenarios.
    uint64_t critical_memory_page_compressions;

    // The nanoseconds in the base unit of time for
    // |pages_decompressed_within_log_time|.
    uint64_t pages_decompressed_unit_ns;

    // How long pages spent compressed before being decompressed, grouped in log
    // buckets. Pages that got evicted, and hence were not decompressed, are not
    // counted here. Buckets are in |pages_decompressed_unit_ns| and round up
    // such that:
    // 0: Pages decompressed in <1 unit
    // 1: Pages decompressed between 1 and 2 units
    // 2: Pages decompressed between 2 and 4 units
    // ...
    // 7: Pages decompressed between 64 and 128 units
    // How many pages are held compressed for longer than 128 units can be
    // inferred by subtracting from |total_page_decompressions|.
    uint64_t pages_decompressed_within_log_time[8];
};
```

Reporting how long pages spend compressed implies that a timestamp will have to
be recorded with each compressed page. This should not produce a significant
impact to potential compression ratios for the benefit of being able to know if
pages are being rapidly decompressed.

#### ZX_INFO_KMEM_STATS_EXTENDED pager bytes

The `ZX_INFO_KMEM_STATS_EXTENDED` query has fields specifically to do with the
amount of bytes in pager backed VMOs. These fields are specifically:

```c
    // The amount of memory committed to pager-backed VMOs.
    uint64_t vmo_pager_total_bytes;

    // The amount of memory committed to pager-backed VMOs, that has been most
    // recently accessed, and would not be eligible for eviction by the kernel
    // under memory pressure.
    uint64_t vmo_pager_newest_bytes;

    // The amount of memory committed to pager-backed VMOs, that has been least
    // recently accessed, and would be the first to be evicted by the kernel
    // under memory pressure.
    uint64_t vmo_pager_oldest_bytes;
```

The proposed implementation will unify the pager backed and anonymous VMO pages
in the page queues, and this will make providing this information impractical.
Therefore the proposal is to redefine these to mean all pageable / reclaimable
memory, and not just that of specifically evictable user pager backed VMOs.
Otherwise the total, newest and oldest definitions will be preserved.

Although reclamation of compressible memory does not translate directly to an
increase in PMM free memory, these queries are not intended to represent an
exact amount of PMM memory that can be reclaimed. Rather they provide an insight
into the relative distribution of memory across ages, and can be used to perform
validations such as `oldest_bytes` goes down and is near zero at device OOM,
etc.

Having multiple fields for user pager backed evictable memory and compressible
anonymous memory is also of questionable value. There will always be
circumstances where being able to distinguish them is valuable, however most of
the time I expect an aggregate report of the two is what would be wanted anyway.

### Disabling compression / latency sensitive VMOs

For latency sensitive applications there needs to be a mechanism for them to
disable compression on VMOs, as no matter how minimal decompression latency is,
it may be beyond their ability to tolerate.

Avoiding reclamation and other kinds of kernel activity that might increase the
latency of memory accesses is an existing problem that is being addressed. As
such, no designs will be proposed here, but this work is a dependency for
compression.

### Extensions

Beyond this initial proposed design there are some explorations and improvements
that could be done. Although these would generally fall under implementation
details, they are included here to motivate the long term suitability of this
design. Improvements that absolutely should be done, preferably prior to use on
products are:

 * Remove separate zero page deduping in favor of compression.

Otherwise optional ideas to explore that may provide useful benefits are:

 * Compress pager backed memory before resorting to evicting it.
 * Support performing compression without discarding uncompressed pages until
   memory pressure.
 * Walk additional old page lists and eagerly compress pages not just in the
   LRU queue.

Further compressed storage and compression algorithm tuning could also be done,
either as direct a evolution, or to provide options to different products. These
might require separate RFCs depending on how invasive or different they are.

## Implementation

The implementation process will be done across multiple stages outlined here.

### Common scaffolding

The common VM system changes to support the aging and tracking of anonymous
pages will be implemented first. These have no behavioral or API changes, but
are the highest risk changes and doing them first allows for:

 * Having the changes in tree and tested for the greatest length of time
   possible.
 * Being able to measure the candidate compression pool (i.e. old anonymous
   pages).

### Kernel implementation

With the common changes done the main kernel implementation can be landed in
tree behind feature flags. This will allow for landing and testing the
implementation of compression to ensure it is stable, before exposing its
existence via any APIs.

### API evolution

Perform API evolutions and struct migrations for the metrics and accounting
changes. Since the kernel implementation is in tree the API changes can be
hooked up, although compression will still be behind a feature flag so aside
from the additional fields regular users will see no functional changes.

### Integrations, extensions and tuning

With the implementation tested and able to report metrics it can be evaluated on
different products to identify any issues, allowing any extensions and tuning
to be performed.

The result of this testing will determine whether:

 * Compression is default on and some products will opt out, or
 * Compression is default off and some products will opt in.

### Extensions and tuning

Parts of the extended design can now be landed, and the general implementation
will be continuously tuned.

## Performance

There are two aspects of performance that need to be evaluated.

### Common overhead

Even if compression is disabled, there are fundamental changes to common VM code
that need to be made, and these will have performance impacts.

It is expected that adding the backlinks to anonymous pages will add measurable
overhead to all VMO clone creation and destruction events. These operations are
not on performance critical paths, as they typically occur during initialization
and tear down steps, but any impact to products should be checked for.

Other VMO paths will incur small overheads to maintain backlinks and update
ages, but this should be in the noise and not result in a measurable impact.

These paths are covered by the existing VMO micro-benchmarks, which will be the
primary validation tool.

### Compression performance

Compression is, at its core, a tradeoff of CPU for memory, and so its
performance can only be evaluated in the context of a product and its needs.

Otherwise the proposed metrics APIs provide the ability to understand what
compression is costing, and the compressions tunables can then be used to
control those costs, with respect to the amount of memory saved.

## Security considerations

### Timing channels

Although compressed pages might have co-located storage, the process of
accessing this storage is defined such that there are no transitive dependencies
on VMOs that could lead to measurable channels. This is intentionally done to
avoid attacks that might be similar to
[memory-deduplication attacks](https://arxiv.org/abs/2111.08553).

Despite a lack of transitive dependencies a timing channel could still
potentially be created if an attacker can control data that gets co-located in
the same page as a secret. This data could be crafted such that compression
succeeds or fails based on the secret, allow the attacker to learn something
about the secret. Whether such an attack is actually viable is unclear, however
if this is a concern the same mechanisms that prevent compression for a latency
sensitive task could also be used.

### System behavior channels

Overall system memory usage can be queried directly via
`ZX_INFO_KMEM_STATS_COMPRESSION`, or inferred by querying an object such as a
VMO and observing if there is a discrepancy between content and committed bytes.
Other stats in `ZX_INFO_KMEM_STATS_COMPRESSION` could also be queried.

This information provides indirect information on overall system behavior.
However, it is not believed that knowing whether or not pages from a VMO you
control were compressed is sufficient to learn anything specific about another
process.

### LZ4

Although the LZ4 library is small in number of files and LoC metrics, it is
still non trivial code written in a low-level style of C and may have bugs. The
library will need a mixture of:

 * Security review for overall suitability.
 * Additional hardening and testing.
 * Potential re-writing of problematic pieces.

A combination of hardening and re-writing could result in porting some or all of
the implementation to safer representations, such as Rust or WUFFS.

## Testing

Similar to performance, there are two dimensions to testing.

### VM correctness

The technical correctness of both the common VM changes, as well as the
compression specific changes, will be evaluated with both unittests and
integration tests.

For the general VM changes that will have effect regardless of compression
enabled or not in kernel unittests or additional core tests will be added as
appropriate.

Integration tests using QEMU runners will be used to run additional tests with
compression enabled.

### System behavior

Although compression is not free, it should not adversely regress any behavior
that is deemed critical by product owners. These behaviors would be product
specific, but an example would be audio skipping, or exacerbated thrashing and
poor user experience under low memory scenarios.

This will involve manually testing products, utilizing tools such as lock
contention tracing, as well as working with product test teams and product
owners to evaluate.

## Documentation

Additions and changes to the `zx_object_get_info` queries will be documented,
along with the additional cmdline options.

## Drawbacks, alternatives, and unknowns

### Unknowns and iteration

The exact benefits and costs of this proposal cannot be fully known until it is
properly implemented and evaluated on real products. Although this proposal
makes an effort to provide a specific and reasonable initial proposal for all
algorithms and structures, these will almost certainly change in response to
real usage data.

### Drawbacks

Primary drawback of this proposal is the complexity added to the VM system in
the kernel. Although any performance aspects of compression can always be
avoided by simply disabling, the existence of compression requires changes to
common infrastructure. There is therefore a permanent complexity and risk
associated with having compression, even if unused.

### User pager compression alternative

Instead of performing compression and decompression in the kernel, it could be
outsourced to user space via the user pager mechanism. In many ways this would
be similar to ZRAM on Linux, where it is implemented by a user space file system
driver. Unfortunately it has many of the same downsides.

#### Benefits

There are two primary reasons to want to do compression in user space:

 1. Remove complicated compression and decompression code from the kernel.
 2. Provide flexibility in user implementation.

Unfortunately (1) is not quite true, since unless a separate hermetic instance
is created for every security domain (however that would be determined) then
this user space process is able to view the anonymous memory of every process in
the system. This would make it incredibly trusted, since any compromise could
view and edit any user memory in the system. As such the bar for trusting the
compression and decompression code here, versus the kernel, would not be lower.

Providing flexibility of implementation is certainly worthwhile, although it
would be more compelling if there were a mechanism to assign a particular
anonymous VMO to a particular compressor. This then might not even need to do
compression, but could be implementing swap or any other strategy.

#### Downsides

The primary downside of user space compression is the constraints it provides on
the kernel. Being able to optimistically compress and decompress pages
(potentially without even discarding the uncompressed ones) gives the kernel
great flexibility of implementation. All such schemes could be implemented via
user pager, but it becomes a more complicated distributed systems problem that
would need very careful API and concurrency reasoning.

Although Zircon is a component system and intended to be cheap to switch user
space tasks, needing to down-call to a user space process to decompress a 4KiB
page will add significant latency and cost. This will incentivize delaying
compression until pages are much older, reducing potential memory savings. For
the existing user pager fault in cases, it is assumed that slow persistent
storage is being queried, hence eviction already skews towards delaying eviction
as far as possible.

Even with user space handling the compression and decompression, all the kernel
changes for page age tracking and triggering of compression still need to be
done. All the tuneables and metrics therefore still need to exist in the kernel.
Additionally, the VM system currently has no way to associate a user pager with
child VMOs and so this would require further changes and re-designs of the VM
system.

### Kernel code interpreter

Instead of a fixed compression algorithm, we could support user supplied
algorithms through user supplied kernel code, a la Linux eBPF. This allows for
flexibility of user implementation, without associated performance impacts of
doing user downcalls.

Although explicit mode transitions are not needed, the kernel will still need
to treat this similar to a user downcall as the performance characteristics will
be unknown, and so will still need to be careful on locking and other
dependencies when invoking the supplied code.

The primary drawback of such an approach is that a VM in the kernel with
sufficient power to produce acceptable performance for compression will require
a substantial implementation that will certainly be complex and greatly increase
the trusted computing base of the kernel.

#### Future

In the future we probably want a way to have anonymous VMOs backed by swap
storage. This would absolutely need to be implemented in user space via a user
pager mechanism. At this point such a swap implementation could do compression,
and not just swap. However, due to the benefits of deep kernel integration of
compression, I see this co-existing with kernel compression, and not replacing
it.

### Accounting alternatives

The proposal currently suggests having a new definition of content bytes/pages
to accompany the existing committed counts. Without changing what is counted,
there are alternative naming options, such as:

 * resident and committed
 * resident and content

Each of these requires changing all existing uses of committed, is a larger
scale change, and creates a greater discontinuity between the two API versions.

### ZX_INFO_KMEM_STATS_EXTENDED pager bytes alternatives

Although the proposed implementation unifies the pager backed and anonymous
queues such that their counts cannot be easily separated, an alternate
implementation could keep the counts separate.

This alternate implementation would have two different sets of counters for each
of the reclaimable queues. A high bit in the `page_queue_priv` stored in the
`vm_page_t` would be used to distinguish which count needs to be updated when
moving the page between queues.

The downsides of this approach are:

* Increased complexity in how `page_queue_priv` is used. It now becomes a packed
field instead of a counter and all reads and writes need to preserve or mask the
extra bit.
* Accessed harvesting, which happens with the arch aspace lock held, cost
increases as the queue information needs to be decoded and the correct counts
updated.
* Multiple count fields and increased complexity in keeping them consistent.

As stated in the proposal, I think for what the pager bytes fields are actually
used for that there is minimal value in having separate counts, given the
additional implementation complexity and runtime costs.

## Experimental evaluation

In order to propose the LZ4 compression algorithm and the compressed storage
strategy, an experimental version of compression was built with sufficient
pieces to:

 * Collect a sample data set of inactive anonymous pages that would be
   compressed.
 * Run different compression algorithms on this data set.

The purpose of this was to evaluate different compression algorithms with:

 * Realistic input data from actual products.
 * In a restricted execution environment that correct matches the kernel.

The second point is particularly important as kernel code runs without support
for FPU/SSE/AVX etc.

Evaluation was performed using a 64MiB input data set, with each 4KiB page being
fed individually to the compressor, as it would be in real operation. The
resulting data is interpreted in the following way:

 * Successful compression means compressed to at least 70% of the original size.
 * Compression time is counted for all pages, including those that ultimately do
   not get stored compressed. This matches real usage where it will not be known
   it advance whether compression will succeed or not.
 * Decompression time is only counted for pages that were successfully
   compressed.
 * Total size counts the storage required for every page, and so pages that were
   successfully compressed have their compressed size added to the count, and
   pages that failed compression get counted as 4KiB.

Rationale for this counting style is that the goal is, ultimately, to free up
memory and use minimal CPU. A compression algorithm that sometimes quickly
compresses a page very well, and otherwise spends a lot of CPU to then fail to
compress is both not actually saving much memory and using lots of CPU.

#### NUC 7

| Algorithm | Compression (MiB/s) | Decompression (MiB/s) | Decompression Worst latency (ns) | Total Size (MiB) |
|-----------|---------------------|-----------------------|----------------------------------|------------------|
| Zstd(-7)  | 752.1470396         | 1446.748532           | 9302.3125                        | 18.61393929      |
| minilzo   | 1212.400326         | 2135.49407            | 6817.839844                      | 16.16410065      |
| lz4(1)    | 1389.675715         | 2920.094092           | 6355.849609                      | 17.21983242      |
| lz4(11)   | 1801.55426          | 3136.318174           | 5255.708984                      | 19.92521095      |
| WKdm      | 1986.560443         | 3119.523406           | 3095.283203                      | 21.98047638      |

#### ARM A53

| Algorithm | Compression (MiB/s) | Decompression (MiB/s) | Decompression Worst latency (ns) | Total Size (MiB) |
|-----------|---------------------|-----------------------|----------------------------------|------------------|
| Zstd(-7)  | 189.3189527         | 486.6744277           | 29856.93359                      | 18.61393929      |
| minilzo   | 317.4546381         | 528.877112            | 15633.99219                      | 16.16410065      |
| lz4(1)    | 294.5410547         | 1127.360347           | 12683.55273                      | 17.21983242      |
| lz4(11)   | 378.2672263         | 1247.749072           | 12452.67676                      | 19.92521095      |
| WKdm      | 337.6087322         | 471.9796684           | 14254.39453                      | 21.98047638      |

#### ARM A73

| Algorithm | Compression (MiB/s) | Decompression (MiB/s) | Decompression Worst latency (ns) | Total Size (MiB) |
|-----------|---------------------|-----------------------|----------------------------------|------------------|
| Zstd(-7)  | 284.3621221         | 623.15235             | 20958.90332                      | 18.61393929      |
| minilzo   | 487.4546545         | 747.9218419           | 16784.83105                      | 16.16410065      |
| lz4(1)    | 441.5927551         | 1159.839867           | 10007.28418                      | 17.21983242      |
| lz4(11)   | 573.7741984         | 1240.964187           | 9987.875                         | 19.92521095      |
| WKdm      | 639.5418085         | 597.6976995           | 13867.47266                      | 21.98047638      |

### Compressed storage

The same 64MiB test data set was used to evaluate the proposed compressed
storage system. In this case it was evaluated in two modes, a two page system
with just a left and a right slot, as well as the proposed three slot system.
Purpose of this was to attempt to quantify the complexity benefit of including
the middle slot over the, truly simple, two slot system.

LZ4, being the proposed compression algorithm, was used to generate the
compressed data to be stored, with the acceleration factor set to 11. This means
that for the 16384 input pages, 12510 produced data that needed to be stored and
3874 were left uncompressed.

| Slots      | Pages for storage | Total MiB | Savings MiB | Compression ratio |
|------------|-------------------|-----------|-------------|-------------------|
| Two-slot   | 6255              | 39.5      | 24.4        | 1.62              |
| Three-slot | 4276              | 31.8      | 32.1        | 2.01              |

Note that 6255 is exactly half of 12510, indicating that there was a buddy
available for every page, meaning the achieved compression ratio was, for the
LZ4 input, the best result for this storage strategy.

## Prior art and references

All major contemporary operating systems (Windows, MacOS, Linux and Linux
derivatives like CastOS etc) support compression of memory as an alternative or
complement to disk swap.

The tri-page storage strategy is inspired by the Linux zbud and z3fold memory
management systems.
