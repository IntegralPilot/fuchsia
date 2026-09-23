# IOBuffer

## Name

IOBuffer - Shared memory endpoint with asymmetric access control and disciplines

## Synopsis

An `IOBuffer` (IOB) is a peered Zircon kernel object designed for
high-throughput, low-latency communication and shared memory transports between
processes. It combines peered session management with multi-region
encapsulation, asymmetric access control, and kernel-mediated access
disciplines.

## Description

An `IOBuffer` always operates as a pair of endpoints (Endpoint 0 and Endpoint
1). It allows two processes to communicate by sharing multiple independent
memory regions (up to a maximum of 64, defined by `ZX_IOB_MAX_REGIONS`), each
configured with specific access permissions and behavior.

### Peered lifetime and signaling

Like Channels and Sockets, `IOBuffer` endpoints are peered.
- The endpoints control the lifetime of the backing memory regions.
- **Reference Tracking**: Active virtual memory mappings created from an
  endpoint count as references alongside open handles to that endpoint.
- **Peer Closure**: When all references (both open handles and active virtual
  memory mappings) to one endpoint close, the system asserts the
  `ZX_IOB_PEER_CLOSED` signal on the opposing endpoint.

### Regions: Private vs. shared

An `IOBuffer` can encapsulate multiple memory regions:
- **Private Regions (`ZX_IOB_REGION_TYPE_PRIVATE`)**: Backed by a private
  `VmObject` uniquely owned by the `IOBuffer` pair. Used for isolated,
  point-to-point communication.
- **Shared Regions (`ZX_IOB_REGION_TYPE_SHARED`)** *(Experimental)*: Points to a
  standalone `shared_region` object. Multiple independent `IOBuffer` pairs can
  reference the same shared region, enabling many-to-one patterns (for example,
  multiple client log writers sending data to a single reader).

### Asymmetric access control

You can configure each region with different permissions for Endpoint 0 and
Endpoint 1. Permissions include:
- **Direct Mapping**: Allows the process to map the region into its virtual
  address space (VMAR) for direct read/write access.
  - `ZX_IOB_ACCESS_EP0_CAN_MAP_READ` / `_WRITE`
  - `ZX_IOB_ACCESS_EP1_CAN_MAP_READ` / `_WRITE`
- **Kernel-Mediated Access**: Restricts direct mapping, requiring all access to
  go through kernel system calls (such as `zx_iob_writev`). This protects
  against Time-of-Check to Time-of-Use (TOCTOU) attacks.
  - `ZX_IOB_ACCESS_EP0_CAN_MEDIATED_READ` / `_WRITE`
  - `ZX_IOB_ACCESS_EP1_CAN_MEDIATED_READ` / `_WRITE`

#### Effective rights and handle interaction

When validating a memory operation, the kernel intersects the region's access
permissions (logical AND) with the endpoint handle rights. Region-level
permissions cannot override handle-level permissions.

- **Map Operation**: The effective read/write rights are `uRn & hRn` and `uWn &
  hWn` respectively, where `u` represents mapping permission and `h` represents
  handle rights.
- **Mediated Operation**: The effective read/write rights are `kRn & hRn` and
  `kWn & hWn` respectively, where `k` represents mediated permission and `h`
  represents handle rights.

#### Mediated directionality vs. absolute permissions

Unlike direct mappings, kernel-mediated access operates in a logical/directional
sense rather than absolute hardware permissions. For example, a logical mediated
read operation (such as retrieving data from a ring buffer) may require the
kernel to write to bookkeeping structures in that same region under the hood.
The kernel permits such internal bookkeeping writes for read-only mediated
endpoints, because the kernel acts as the trusted mediator enforcing the logic.

### Memory access disciplines

Disciplines define the structured memory layout and behavior for
kernel-mediated operations within a region:
- **None (`ZX_IOB_DISCIPLINE_TYPE_NONE`)**: Free-form raw byte buffer. No
  kernel-mediated operations.
- **ID Allocator (`ZX_IOB_DISCIPLINE_TYPE_ID_ALLOCATOR`)** *(Experimental)*: A
  thread-safe structure mapping sized data blobs to sequentially allocated
  numeric IDs. Useful for string interning in tracing.
- **Mediated Write Ring Buffer
  (`ZX_IOB_DISCIPLINE_TYPE_MEDIATED_WRITE_RING_BUFFER`)** *(Experimental)*: A
  circular ring buffer designed for concurrent, kernel-mediated writes by
  multiple clients and a single userspace reader (for example, high-efficiency
  system logging).

### Mapping via VMAR

The system maps IOBuffer regions into a VMAR via `zx_vmar_map_iob`. Only the
following VMAR options are supported:
- `ZX_VM_SPECIFIC`
- `ZX_VM_SPECIFIC_OVERWRITE`
- `ZX_VM_OFFSET_IS_UPPER_LIMIT`
- `ZX_VM_PERM_READ`
- `ZX_VM_PERM_WRITE`
- `ZX_VM_MAP_RANGE`

Any other VMAR options return `ZX_ERR_INVALID_ARGS`.

### Querying object properties and regions

IOBuffers support standard property queries via `zx_object_get_info`.

#### `ZX_INFO_IOB`
Returns information about the overall `IOBuffer` instance using `zx_iob_info_t`:
- ``options``: The options used at creation.
- ``region_count``: The number of memory regions encapsulated.

#### `ZX_INFO_IOB_REGIONS`
Returns information about each region as an array of `zx_iob_region_info_t`.
- **Access Bit Swapping**: When returned, the kernel swaps the access modifier
  bits so that Endpoint 0 access bits reflect the rights of the endpoint handle
  executing the query, and Endpoint 1 access bits reflect the peer's rights.
  This allows endpoint-agnostic libraries to validate permissions dynamically.

#### `ZX_INFO_PROCESS_VMOS`
The kernel reports the memory objects backing private IOB regions under this
topic like standard VMOs. By default, backing VMOs share the name of the parent
`IOBuffer`.

## Rights

An `IOBuffer` handle has the following rights by default:
- `ZX_RIGHT_TRANSFER`
- `ZX_RIGHT_DUPLICATE`
- `ZX_RIGHT_WAIT`
- `ZX_RIGHT_INSPECT`
- `ZX_RIGHT_READ`
- `ZX_RIGHT_WRITE`
- `ZX_RIGHT_MAP`
- `ZX_RIGHT_SIGNAL`
- `ZX_RIGHT_SIGNAL_PEER`
- `ZX_RIGHT_GET_PROPERTY`
- `ZX_RIGHT_SET_PROPERTY`

## Properties

IOBuffers support the following properties:
- `ZX_PROP_NAME`: Used for diagnostics and attributing memory.

## Signals

The system can set the following signals for an `IOBuffer` endpoint:

- **ZX_IOB_PEER_CLOSED**: The peer endpoint closed (including all handles and
  active mappings).
- **ZX_IOB_SHARED_REGION_UPDATED** *(Experimental)*: Raised when a shared region
  has been updated by a mediated write.

## Syscalls

- [`zx_iob_create()`] - create a new peered IOBuffer pair
- [`zx_iob_create_shared_region()`] *(Experimental)* - create a standalone
  shared region
- [`zx_iob_writev()`] - perform a kernel-mediated write to a region
- [`zx_iob_allocate_id()`] *(Experimental)* - allocate an ID in an ID Allocator
  region
- [`zx_vmar_map_iob()`] - map an IOBuffer region into a VMAR

[`zx_iob_create()`]: /reference/syscalls/iob_create.md
[`zx_iob_create_shared_region()`]: /reference/syscalls/iob_create_shared_region.md
[`zx_iob_writev()`]: /reference/syscalls/iob_writev.md
[`zx_iob_allocate_id()`]: /reference/syscalls/iob_allocate_id.md
[`zx_vmar_map_iob()`]: /reference/syscalls/vmar_map_iob.md
