# FDomain Protocol

An FDomain is a collection of handles that can be manipulated remotely.
Consequently the FDomain protocol is a protocol that allows a client to request
certain actions be performed on certain handles within an FDomain.

## Protocol Design

The FDomain protocol is a regular FIDL protocol, except that it never includes
handles, making it possible to transmit it over a non-channel medium, or off the
device entirely. It otherwise relies on normal FIDL mechanisms, including
standard FIDL transaction headers and the use of an open protocol and flexible
methods for API evolution and compatibility.

### Two-way methods only

The FDomain protocol only uses two-way methods which have the option to return
an error. This allows unknown method errors to always be returned to the client
in the event of an API mismatch. (FIDL allows one way methods to fail silently,
and can't report a proper unknown error code if the original method doesn't
normally report errors).

## Protocol Primitives

FDomain defines a small set of core types that are used throughout the protocol.

### Handle IDs

Handles within FDomain are referred to by an ID. IDs are 32-bit integers, not
unlike the handles themselves, however handle IDs do not correlate to the actual
integer value of handles.

When we pass around handle IDs in FDomain we use the newtype wrapper `Hid` to
make the semantics clear.

FDomain supports operations that create handles, and we would like to support
pipelining for those operations. For example, we'd like the client to be able to
create a socket and then immediately try to send data through that socket
without waiting for a response to the creation request. For this to work, the
client has to know the ID of the new socket before the creation request returns.
We solve this by allowing the client to specify the IDs of the new handles created
by a request. When the client is specifying handle IDs to be populated by a
request we use the `NewHid` newtype wrapper.

There are operations which may create new handles in the FDomain where we cannot
give the client the opportunity to specify what the new IDs should be. Most
notably, reading from a channel may produce a message with an unknown number of
handles. In these cases the handle ID is assigned by the FDomain itself. To
prevent collisions between handle IDs assigned by the FDomain and handle IDs
provided by the client, the most significant bit of the handle ID sent in a
`NewHid` must always be zero (and the most significant bit of a handle ID
created by the FDomain will *never* be zero).

#### Allocation

Handle IDs, whether allocated by the client or FDomain, should be chosen
randomly within the space of allowable values.

Handle IDs should not be reused. If a `NewHid` is sent as part of a request
which fails, the ID in that `NewHid` should not be reused. The FDomain may
return the `bad_hid` error variant to the client if it detects a handle ID being
reused by the client.

### Handle Info and Disposition

FDomain provides its own versions of `zx_handle_info_t` and
`zx_handle_disposition_t` which contain handle IDs instead of handles. These are
named, as one might expect, `HandleInfo` and `HandleDisposition`. These structs
use the `Rights` and `ObjectType` types for metadata, whereas for the
handle operation in `HandleDisposition` is stored using an FDomain specific type
`HandleOp`. These are used where we'd expect their Zircon counterparts to be
used when operating on local handles directly. Consult the
[Zircon documentation](https://fuchsia.dev/reference/syscalls) for their
semantics.

### Signals

FDomain provides a `Signals` type which is a `bits` and is analogous to
`zx_signal_t`. It is currently bit-compatible with `zx_signals_t` as well but is
defined separately so it can be versioned independently.

To prevent incompatibility, the implementation should not rely on the bit
compatibility here and should always manually translate to and from
`zx_status_t`, validating along the way. This will create code that will be
disrupted by a syscall API change and signal that additional compatibility work
is needed.

### Rights

FDomain provides a `Rights` type which is a `bits` and is analogous to
`zx_rights_t`. It is currently bit-compatible with `zx_rights_t` as well but is
defined separately so it can be versioned independently.

To prevent incompatibility, the implementation should not rely on the bit
compatibility here and should always manually translate to and from
`zx_rights_t`, validating along the way. This will create code that will be
disrupted by a syscall API change and signal that additional compatibility work
is needed.

### ObjType

FDomain provides an `ObjType` type which is an `enum` and is analogous to
`zx_obj_type_t`. It is currently bit-compatible with `zx_obj_type_t` as well but
is defined separately so it can be versioned independently.


To prevent incompatibility, the implementation should not rely on the bit
compatibility here and should always manually translate to and from
`zx_obj_type_t`, validating along the way. This will create code that will be
disrupted by a syscall API change and signal that additional compatibility work
is needed.

### Errors

The FDomain protocol has a global `Error` union which provides an error type for
returning from methods within the protocol. Most of the variants we will
explain as they come up, but here are a few globally relevant ones:

* `target_error` - Contains an `int32` presumed to be a `zx_status_t`, and
  indicates that an operation on a handle failed for reasons relating to the
  handle itself rather than the FDomain or the protocol state. In short this
  indicates that the actual system call implied by the operation requested
  returned an error.
* `bad_hid` - indicates that the client used a Handle ID which did not refer to
  a handle in the FDomain. Contains the unwrapped `uint32` handle ID that was
  given.
* `wrong_handle_type` - indicates we expected a handle of some type (e.g. a
  socket) and the client gave an ID referring to a handle of a different type
  (e.g. a channel). Contains `ObjType`s for both the expected and actual
  type.
* `bad_new_hid` - Indicates we gave a `NewHid` that either referred to an
  existing handle, or had the most significant bit set.

Note that in the case of `target_error`, a major change to the schema of values
used in `zx_status_t` could cause a breakage in error reporting. Such changes
are incredibly rare, and are usually additive, which shouldn't disrupt
compatibility at all. We've decided the risk is low enough that we will risk a
degraded error reporting experience for legacy tool users to keep the API simple
here.

## Core Methods

The FDomain protocol has a series of core methods that are generally useful for
operating on any type of handle or otherwise manipulating the FDomain, and then
composes several sub-protocols with methods specific to dealing with certain
types of handle. These are the core methods:

### `Namespace`

The `Namespace` method takes a `NewHid` and creates a channel at that ID which
points to a `fuchsia.io.Directory`. What that directory contains is up to the
service exposing the FDomain. This the mechanism by which we can "bootstrap" an
FDomain; it allows us to get an initial set of handles which connect to other
services we might want to communicate with.

### `Close`

`Close` takes a list of `Hid`s and closes the associated handles. The `Hid`s are
no longer valid after this operation.

### `Duplicate`

`Duplicate` takes a `Hid`, a `NewHid`, and a `Rights` and duplicates the
handle at `Hid`, placing the new handle at `NewHid`.  The `Rights` indicates
the rights for the new handle. The specific semantics of the duplication are
identical to `zx_handle_duplicate`.

### `Replace`

`Replace` takes a `Hid`, a `NewHid`, and a `Rights` and destroys the handle
associated with `Hid`, placing a new handle to the same object at `NewHid`,
which now has the rights given. The semantics are identical to the syscall
`zx_handle_replace`.

### `Signal`

`Signal` takes a `Hid` and two `Signals` values, a "set" list and a "clear"
list, and asserts and de-asserts those signals respectively on the handle
associated with the `Hid`. Semantics are identical to `zx_object_signal`.

### `SignalPeer`

`SignalPeer` takes a `Hid` and two `Signals` values, a "set" list and a "clear"
list, and asserts and de-asserts those signals respectively on the handle
*peered with* the handle associated with the `Hid`. Semantics are identical to
`zx_object_signal_peer`.

### `WaitForSignals`

`WaitForSignals` takes a `Hid` and a `Signals` value and hangs returning until
one of the signals given is asserted on the handle associated with the `Hid`. It
returns a `Signals` value indicating which signal or signals were asserted.

### `AcknowledgeWriteError`

FDomain wants to enable pipelining for transactions. That means if you submit a
request to write from a handle, you can submit a second request to write more
data before the first request returns.

This creates a potential to produce inconsistent writes. Consider a client that
submits three write requests, A, B, and C. Suppose request B experiences a
transient failure, but A, and C succeed. This means the receiver will see
message A followed by message C. Message B could then be re-submitted, but the
messages will arrive in the order A, C, B, which may be undesirable if message
ordering is important.

To fix this, Playground has the notion of "write-locked" handles. A particular
request may specify in its documentation that it write-locks the handle on
error. This makes further write operations on that handle fail. So in our
example, if A succeeded, and B failed transiently, C would also fail, returning
the `error_pending` variant of our `Error` union.

`AcknowledgeWriteError` takes a `Hid` and removes the write-locked state from
the associated handle, returning it to normal operation. If the handle is not
write-locked, `AcknowledgeWriteError` will return the `no_error_pending` error
variant.

## Events

The FDomain protocol composes an `Event` sub-protocol for dealing with Event
handles. This protocol exposes one method, `CreateEvent`, which takes a `NewHid`
and associates it to a newly-created event handle.

## Event Pairs

The FDomain protocol composes an `EventPair` sub-protocol for dealing with Event
Pair handles. This protocol exposes one method, `CreateEvent`, which takes an
array of two `NewHid`s and associates them to a newly-created pair of event pair
handles.

## Sockets

The FDomain protocol composes a `Socket` sub-protocol for dealing with Socket
handles.

The `SocketProtocol` has the following methods:

### `CreateSocket`

`CreateSocket` takes an options parameter and an array of two `NewHid`s. It
associates the two new ids with a pair of sockets.

The options parameter has the type `SocketType` which is defined alongside the
`Socket` protocol and has two values: `STREAM` and `DATAGRAM`. This can be used
to select stream or datagram semantics for the new socket.

### `SetSocketDisposition`

`SetSocketDisposition` takes an `Hid` and two `SocketDisposition` arguments.

`SocketDisposition` is an enum defined alongside the `Socket` protocol and has three values:
`NO_CHANGE`, `WRITE_ENABLED`, and `WRITE_DISABLED`.

`SetSocketDisposition` changes the disposition of the socket associated with the
`Hid`, and the disposition of that socket's peer.

## VMOs

The FDomain protocol composes a `Vmo` sub-protocol for dealing with Virtual
Memory Object (VMO) handles.

The `Vmo` protocol has the following methods:

### `CreateVmo`

`CreateVmo` takes a `size`, `options` (`VmoOptions` bits: `RESIZABLE`, `TRAP_DIRTY`, `UNBOUNDED`),
and a `NewHid`. It creates a new VMO of the given size with the requested options in the FDomain
and associates it with the `NewHid`.

### `ReadVmo`

`ReadVmo` takes a `Hid`, an `offset`, and a `size`. It reads `size` bytes from the
VMO at `offset` and returns the data read.

### `WriteVmo`

`WriteVmo` takes a `Hid`, an `offset`, and a `data` byte vector. It writes the data
to the VMO at `offset`.

### `GetVmoSize`

`GetVmoSize` takes a `Hid` and returns the size of the VMO in bytes.

### `SetVmoSize`

`SetVmoSize` takes a `Hid` and a `size`. It sets the size of the VMO to the given
value.

### `GetVmoStreamSize`

`GetVmoStreamSize` takes a `Hid` and returns the stream size of the VMO in bytes.

### `SetVmoStreamSize`

`SetVmoStreamSize` takes a `Hid` and a `size`. It sets the stream size of the VMO
to the given value.

