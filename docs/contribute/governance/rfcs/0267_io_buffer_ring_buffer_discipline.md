<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0267" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}

<!-- mdformat on -->

## Problem Statement

We would like a more efficient logging mechanism to improve CPU usage by the
logging system, and to improve scheduling pressure. Memory usage is also a
concern but not the primary focus of this RFC.

## Summary

One of the primary use cases for introducing IO Buffers was to support
[more efficient logging](0218_io_buffer.md#system_logging). This RFC introduces
a new ring buffer discipline with the goal of improving CPU usage and scheduling
pressure by the logging system.

## Stakeholders

*Facilitator:* jamesr@google.com

*Reviewers:* adanis@google.com, eieio@google.com, gmtr@google.com,
miguelfrde@google.com

*Consulted:* Kernel and Diagnostics teams

*Socialization:*

This proposal was discussed with the kernel and diagnostics teams before this
RFC.

## Requirements

*   Archivist must know the identity of logging components.
*   Logging must be supported prior to Archivist running.
*   This RFC should not significantly increase total memory use.

## Background

Logging clients currently use `fuchsia.logger.LogSink`:

```
    strict ConnectStructured(resource struct {
        socket zx.Handle:SOCKET;
    });
```

Logging messages are sent as datagram messages on the socket. Archivist services
many sockets. For each socket it reads messages and writes them into a ring
buffer:

1.  Client writes to socket.
1.  Kernel copies message into socket buffer.
1.  Archivist wakes.
1.  Archivist copies message from kernel socket buffer into ring buffer.

## Design

After this RFC is implemented, writing a log becomes:

1.  Client performs a mediated write to an IO Buffer.
1.  Kernel copies message into an IO Buffer region.

A new ring buffer discipline will be added:

```c++
#define ZX_IOB_DISCIPLINE_TYPE_MEDIATED_WRITER_RING_BUFFER ((zx_iob_discipline_type_t)(2u))
```

```c++
typedef struct zx_iob_discipline_mediated_writer_ring_buffer {
  uint64_t tag;
  uint64_t reserved[7];
} zx_iob_discipline_mediated_writer_ring_buffer_t;
```

This discipline will initially support concurrent kernel mediated writes with a
single userspace reader. Userspace writes and kernel mediated reads will *not*
be supported. This discipline will initially only be usable with shared regions
(discussed below). The reserved fields might be used in future to configure
behaviour, such as enabling or disabling userspace writes.

The discipline allows specification of a tag which the kernel will write to the
ring buffer and allows Archivist to know the identity of the client (it can
store a mapping from tag to component identity).

The first page of a region using this discipline will contain a header
comprising:

```
uint64_t head;
uint64_t tail;
```

The rest of the region (starting from the second page) will be used as a ring
buffer which means that the region must be at *least* two pages and the ring
buffer will be a power-of-two in size. Starting the ring buffer on a page
boundary allows clients to map the beginning of the ring buffer at the end which
can make dealing with wrapping easier. It also allows modulo two arithmetic to
be used on the head and tail values which, in theory, can be more performant
than otherwise. And lastly, it provides room for growth for additional metadata
in future, should it be required. The downside is that it wastes some space.

Kernel mediated writes will be supported via a new syscall:

```c++
// Performs a mediated write to the specified IO Buffer region.
//
// The maximum size of the data to be written is 65,535 bytes.
//
// |options| is reserved for future use and must be zero.
// |region_index| specifies which region to write to. It must use a discipline
//     that supports mediated writes.
//
// * Errors
//
// `ZX_ERR_ACCESS_DENIED`: Input handle does not have sufficient rights or |data| is
//     not readable.
// `ZX_ERR_BAD_STATE`: The ring buffer is in an invalid state (e.g. tail > head).
// `ZX_ERR_BAD_TYPE`: Input handle is not an IO Buffer.
// `ZX_ERR_INVALID_ARGS`: |options| or |region_index| are invalid, or there is an
//     attempt to write more than 65,535 bytes.
// `ZX_ERR_NO_SPACE`: There is no space in the ring buffer.
zx_status_t zx_iob_writev(
    zx_handle_t iob_handle, uint64_t options, uint32_t region_index,
    const zx_iovec_t* vectors,  size_t num_vectors);
```

This syscall will initially only work for the new ring buffer discipline. All
writes to the ring buffer will consist of a 8 byte header comprising the tag of
the IO Buffer (8 bytes), followed by a 8 byte length indicating the number of
bytes that follow. All writes will be padded to maintain 8 byte alignment,
although the length need not be a multiple of 8 bytes.

The `head` and `tail` pointers will be updated atomically with appropriate
barriers. Initially, concurrent writes will be supported by using locks within
the kernel which is possible because only mediated writes are supported. The
`head` and `tail` pointers will only ever be incremented; as they are 64 bit,
they will not wrap in our lifetime. The ring buffer offset will be determined
using modulo arithmetic.

Only the kernel will increment `head`. Only userspace will increment `tail`.
Userspace will be responsible for maintaining sufficient space within the ring
buffer. If there is insufficient space, `zx_iob_writev` will fail with
`ZX_ERR_NO_SPACE`. For Archivist, a consequence of this is that the ring buffer
will need to be larger than it currently is because Archivist must maintain
sufficient free space such that logs are rarely dropped.

Each logging client will need its own IO Buffer that shares a region using the
new ring buffer discipline. To support this a new Kernel object will be
introduced to represent a shared region:

```c++
// Creates a shared region that can be used with an IO Buffer.
//
// |size| is the size of the shared region.
// |options| is reserved for future use and must be zero.
//
// * Errors
//
// `ZX_ERR_INVALID_ARGS`: The size is not a multiple of the page size.
zx_status_t zx_iob_create_shared_region(uint64_t options, uint64_t size, zx_handle_t* out);
```

Many IO Buffers can be created using this shared region. `zx_iob_region_t` will
be extended:

```c++
#define ZX_IOB_REGION_TYPE_SHARED ((zx_iob_region_type_t)(1u))

// If the type is ZX_IOB_REGION_TYPE_SHARED, the size comes from the shared region and
// |size| must be zero.
struct zx_iob_region_t {
  uint32_t type;
  uint32_t access;
  uint64_t size;
  zx_iob_discipline_t discipline;
  union {
    zx_iob_region_private_t private_region;
    zx_iob_region_shared_t shared_region;
    uint8_t max_extension[4 * 8];
  };
};

// |options| is reserved for future use and must be zero.
// |shared_region| is a handle (not consumed) referencing a shared region created with
// |zx_iob_create_shared_region|. |padding| must be zeroed.
struct zx_iob_shared_region_t {
  uint32_t options;
  zx_handle_t shared_region;
  uint64_t padding[3];
};
```

After successfully writing to the ring buffer, the shared region will have its
`ZX_IOB_SHARED_REGION_UPDATED` signal strobed. This will allow Archivist to know
when new messages have been written to the shared region (if it needs to e.g.
there are downstream log readers).

Archivist can monitor when logging clients go away by monitoring signal
`ZX_IOB_PEER_CLOSED` on its endpoint for each of the clients.

### Read Threshold

Similar to `ZX_SOCKET_READ_THRESHOLD`, there will be a property
`ZX_IOB_SHARED_REGION_READ_THRESHOLD` that can be set that will cause the
`ZX_IOB_SHARED_REGION_READ_THRESHOLD` signal to be strobed when a threshold is
crossed after a write. Note that the signal can only be strobed, not asserted,
because the kernel cannot know when to deassert the signal.

### Memory Order and Writer Synchronization

The following are the minimum memory order requirements for the atomic
operations:

#### Initialization

No synchronization is required when first initializing the ring buffer. The ring
buffer will start with all header fields zeroed. No support for reinitialisation
is required.

#### Write

Concurrent writes will be synchronized using a lock in the kernel.

1.  Read the head with relaxed semantics and tail with acquire semantics.
1.  Perform validation and space checks.
1.  Write the message with normal writes. This will be performed as an
    opportunistic copy. If a fault occurs, the lock will be dropped, the fault
    will be handled and then the operation will be retried from the first step.
1.  Increment head with release semantics.

A badly behaved client would have the potential stall writes for all clients if
it were to, say, set the source of memory for the message to a misbehaving pager
source. See the section below on Badly Behaved Clients.

#### Read

1.  Read the tail with relaxed semantics and head with acquire semantics.
1.  Perform checks on head and tail. Fail on error or if no message is present.
1.  Read the message with normal reads.
1.  Increment the tail with release semantics.

### Client Side Buffering

Some components currently start before Archivist is running. If Archivist is
responsible for creating the IOBuffer pair, then, to avoid deadlocks, the client
side will need to buffer logs until it receives the IOBuffer. This may have
implications for short-lived components that start and stop before Archivist has
started (since, without care, log messages could be dropped).

Alternatively, Component Manager would need to be made responsible for creating
the IOBuffer objects.

One of these two approaches will need to be taken to adopt this RFC, but which
is chosen is beyond the scope of this RFC.

This assumes that we continue with a single, common, ring buffer as is currently
the case, but it is possible for us to consider using multiple ring buffers
created and used by different components (for example to address isolation,
allocation and attribution concerns). This is out of scope for this RFC except
to say that such solutions shouldn't be prohibited.

## Badly Behaved Clients

Badly behaved clients have the potential to disrupt other clients by writing so
many logs that it forces other logs to be dropped, or, for example, by writing
from pager backed memory that they refuse to provide the backing for.

To some extent, these issues already exist: components can already send so many
logs that it causes other logs to be dropped, and they can hog CPU and memory to
such an extent that it degrades the rest of the system.

We propose that we deal with these issues as we do now: either as bugs that need
to be fixed, or bad applications that the user must kill (where the product
enables this).

## Implementation

`zx_iob_writev` needs to be stabilized before we can use any part of this RFC
for logging. As this is the only new API that clients need to use, all other
proposed changes will initially be part of the Zircon kernel's NEXT vdso (which
can be used by Archivist). This will allow us to iterate on the design should we
need to.

There will need to be changes to logging protocols to support this change, but
these are out of scope for this RFC.

## Performance

This should improve logging efficiency: messages can be written directly to the
ring buffer without Archivist needing to wake. Existing Archivist and system
benchmarks will be monitored.

## Security considerations

N/A

## Privacy considerations

N/A

## Testing

Unit tests will be added. Archivist has suitable integration and end-to-end
tests that will be used.

## Documentation

The kernel syscall documentation will be updated.

## Future Possibilities

1.  We might want to look at supporting blocking writes. We believe this is not
    currently required, but if it were, then we could provide an option to
    `zx_iob_write` to make the write blocking.

1.  The logging format currently includes a timestamp which the client
    generates. We could add options to allow the timestamp to be generated by
    the kernel, which would allow the receiver to trust the timestamp, at the
    expense of some accuracy. It might also be possible to guarantee some
    ordering, although this might further weaken the accuracy (due to lock
    contention and need to retry in the writer).

1.  To address isolation concerns, we could consider supporting multiple ring
    buffers for different subsystems.

1.  We might add options to support kernel generated ids in future, such as a
    process ID, a thread ID, or something equivalent that doesn't leak such
    details to unprivileged users.

## Drawbacks, alternatives, and unknowns

A number of alternatives were discussed prior to this RFC including:

1.  We could have a single IO Buffer with a new tagged writer kernel object i.e.
    a many-to-one arrangement. This has some advantages, but was discounted
    because it leads to a complicated scheme for tracking tags. Archivist needs
    to know when a logger goes away: the proposal in this RFC leans on the
    kernel's existing peered dispatcher model for this, whereas a single IO
    Buffer with a tagged writer would require something different.

1.  We could derive the tag from a property on the calling thread. Tags would be
    inherited from processes and jobs. This approach was discounted because
    runners do not necessarily have to run components in a separate thread.

1.  An approach that uses streams was considered, but that was discounted
    because of concerns regarding lock contention since streams operate on paged
    memory whereas IO Buffers can operate on pinned memory.

1.  We could associate a tag with a handle which would mean we could use a
    single IO Buffer and then just pass different handles to different writers.
    This would have required significant restructuring of the Zircon kernel to
    support this, for an arguably worse API.

1.  We could have the kernel use the IOBuffer koid as the tag. This complicates
    the tracking that Archivist would need to do, as it would need to track per
    IOBuffer counts of messages in rather than per component counts of messages.
