<!-- Generated with `fx rfc` -->
<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0265" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Problem Statement

This RFC is motivated by two separate use cases, Starnix Suspension and
Timestamped Graphics Fence.  While Starnix Suspension is the driving use case,
Timestamped Graphics Fence is an important secondary use case.

### Starnix Suspension

We need a way for the Starnix Kernel and Starnix Runner to coordinate and to
prevent suspension when there are pending messages from Fuchsia platform
components.

There are three processes involved:

1. Some Fuchsia platform component, which is sending a message on a channel.

2. Starnix Kernel, which needs to read and act on that message.

3. Starnix Runner, whose role is described below.

The Starnix Runner is responsible for suspending the Starnix Kernel by placing
all of its threads into the `ZX_THREAD_SUSPENDED` state and observing messages
sent by Fuchsia platform components to the Starnix Kernel.  The Starnix Runner
acts as a proxy, receiving messages on behalf of the Starnix Kernel and then
forwarding them if the Starnix Kernel is not suspended, or resuming and then
forwarding if it is suspended.

We want to make sure we don't have any "lost wakeups" so the Starnix Runner and
Starnix Kernel must agree on whether there are any outstanding or "in-flight"
messages that should prevent the Starnix Kernel from being suspended.

Currently, the Starnix Runner and Starnix Kernel coordinate using a single
`zx::event`.  This event is used to indicate that there is a message in-flight.
Using an event in this way works and prevents lost wakeups.  However it comes
with the limitation that at most one message may be proxied at a time.  Without
this limitation, the Starnix Runner can't tell whether the Starnix Kernel has
processed *all* the in-flight messages or just the ones that it has already
received.

We want to remove the limitation and support multiple in-flight messages.  To
support multiple in-flight messages, we need some way to count them so that we
can suspend only once the count has reached zero.

A `zx::event` alone is insufficient when there are be multiple in-flight
messages.  However, a `zx::event` plus a shared integer would be sufficient.  If
the Starnix Runner and Starnix Kernel shared an address space, we could simply
store the integer in memory.  However, these processes do not share address
space so we'd need to store the integer in some other shared resource, like a
VMO.  Given that these processes are in the same logical trust domain,
coordinating using a VMO is entirely doable.  However, we can do better and
simplify things a bit if we had a new kind of Zircon object that acted like an
event with a count.

### Timestamped Graphics Fence

[ZirconVmoSemaphore] is used to implement a timestamped graphics fence.  It acts
like a `zx::event` plus a timestamp indicating the time at which the object was
signaled.  The graphics stack creates several of these objects for each frame.
Each one is backed by a VMO, so each one occupies a page of memory (plus some
overhead).  Using a VMO to implement a timestamped graphics fence works, but is
somewhat inefficient.  We can do better if we had an object that acted like a
`zx::event` plus timestamp.

See also [VkFence].

## Summary

We propose creating a new Zircon object, `zx::counter`, that holds an integer,
can be incremented/decremented, and asserts a signal when the value is greater
than zero or less-than-or-equal-to zero.

## Stakeholders

The primary stakeholders are Zircon, Starnix, and Graphics.

_Facilitator:_ davemoore@google.com

_Reviewers:_ emircan@google.com, lindkvist@google.com, mcgrathr@google.com

_Consulted:_ adanis@google.com, eieio@google.com, rudymathu@google.com

_Socialization:_ Variations of this proposal have been discussed with several
folks over email prior to the first draft of the RFC.  See also internal
discussion in [go/zx-counter].

## Goals and Requirements

1. Solve the problems at hand - Enable Starnix Kernel and Starnix Runner to
coordinate and prevent suspension when there are pending messages from Fuchsia
platform components.  Enable the graphics stack to replace the somewhat
heavy-weight VMO-based [ZirconVmoSemaphore] with a more efficient
synchronization tool.

2. Keep it simple - Whatever is used to solve the problem at hand should be kept
simple.  Don't build out more functionality than necessary.  If more use
cases arise, we can evolve the solution (or create different solutions!) for
those future use cases.

## Design

We introduce a new Zircon object, Counter.  Counter is like an event, but with
a signed 64-bit integer that can be incremented, decremented, read, or written.

When a Counter's value is greater than zero, the signal `ZX_COUNTER_POSITIVE` is
asserted and `ZX_COUNTER_NON_POSITIVE` is deasserted.  When the value is less
than or equal to zero, `ZX_COUNTER_POSITIVE` is deasserted and
`ZX_COUNTER_NON_POSITIVE` is asserted.

While the Starnix Suspension use case is satisfied by increment and decrement
operations that assert/desassert the signals, the Timestamped Graphics Fence use
case needs read and write operations.  A counter used as a fence will start with
the value zero, representing an unsignaled fence.  A positive value represents a
signaled fence with the value itself being the time at which the fence was
signaled.

To signal the fence, the signaling component in the graphics stack will set the
counter's value to the current time (think `zx_instant_mono_t`), using the write
operation.  When the counter's value is changed from zero to this positive
value, `ZX_COUNTER_POSITIVE` will be asserted.  The signalee component in the
graphics stack can then read the counter's value to determine the time at which
the fence was signaled.

To support both use cases, we add four new syscalls (`zx_counter_create`,
`zx_counter_add`, `zx_counter_read`, `zx_counter_write`) and two additional
signals:

```
// Asserted when a counter's value is less than or equal to zero.
#define ZX_COUNTER_NON_POSITIVE __ZX_OBJECT_SIGNAL_4

// Asserted when a counter's value is greater than zero.
#define ZX_COUNTER_POSITIVE __ZX_OBJECT_SIGNAL_5
```

Counters are created with `zx_counter_create`:

```
## Summary

Create a counter.

## Declaration

zx_status_t zx_counter_create(uint32_t options, zx_handle_t* out);

## Description

zx_counter_create() creates a counter, which is an object that encapsulates
a signed 64-bit integer value that can be incremented, decremented, read, or
written.

When the value is greater than zero, the signal ZX_COUNTER_POSITIVE is
asserted.  Otherwise ZX_COUNTER_NON_POSITIVE is asserted.  Exactly one of
these two signals is always asserted, and never both at once.

The newly-created handle will have rights ZX_RIGHTS_BASIC, ZX_RIGHTS_IO, and
ZX_RIGHT_SIGNAL.  The value will be zero and the signal
ZX_COUNTER_NON_POSITIVE will be asserted on the newly-created object.

## Rights

Caller job policy must allow ZX_POL_NEW_COUNTER.

## Return value

zx_counter_create() returns ZX_OK and a valid event handle (via *out*) on
success.

On failure, an error value is returned.

## Errors

ZX_ERR_INVALID_ARGS  *out* is an invalid pointer, or *options* is non-zero.

ZX_ERR_NO_MEMORY  Failure due to lack of memory.
There is no good way for userspace to handle this (unlikely) error.
In a future build this error will no longer occur.
```

Counters may be incremented or decremented using `zx_counter_add`:

```
## Summary

Add an amount to a counter.

## Declaration

zx_status_t zx_counter_add(zx_handle_t handle, int64_t amount);

## Description

zx_counter_add() adds amount to the counter referenced by handle.

After the result of the addition, if the counter's value is:

* less than or equal to zero - ZX_COUNTER_NON_POSITIVE will be asserted
and ZX_COUNTER_POSITIVE will be deasserted.

* greater than zero - ZX_COUNTER_POSITIVE will be asserted and
ZX_COUNTER_NON_POSITIVE will be deasserted.

## Rights

handle must have both ZX_RIGHT_READ and ZX_RIGHT_WRITE.  Because a counter's
value could be determined by checking for ZX_ERR_OUT_OF_RANGE on a series of
carefully crafted zx_counter_add calls, there is no way to create a counter
that cannot be read, but which can be added.

## Return value

zx_counter_add() returns ZX_OK on success.

On failure, an error value is returned.

## Errors

ZX_ERR_WRONG_TYPE  if handle is not a counter handle.

ZX_ERR_ACCESS_DENIED  if handle does not have ZX_RIGHT_WRITE.

ZX_ERR_OUT_OF_RANGE  if the result of the addition would overflow or underflow.
```

Counters may be read using `zx_counter_read`:

```
## Summary

Read the value of a counter.

## Declaration

zx_status_t zx_counter_read(zx_handle_t handle, int64_t* value);

## Description

zx_counter_read() reads the value of the counter referenced by handle into the
integer value points at.

## Rights

handle must have ZX_RIGHT_READ.

## Return value

zx_counter_read() returns ZX_OK on success.

On failure, an error value is returned.

## Errors

ZX_ERR_WRONG_TYPE  if handle is not a counter handle.

ZX_ERR_ACCESS_DENIED  if handle does not have ZX_RIGHT_READ.

ZX_ERR_INVALID_ARGS  if value is an invalid pointer.
```

Counters may be written using `zx_counter_write`:

```
## Summary

Write the value to a counter.

## Declaration

zx_status_t zx_counter_write(zx_handle_t handle, int64_t value);

## Description

zx_counter_write() writes value to the counter referenced by handle,
asserting/deasserting signals as necessary.

Because concurrent operations on a counter may be interleaved with one another,
an implementation of a "counting semaphore" synchronization protocol should use
zx_counter_add() instead of a sequence of zx_counter_read(), modify,
zx_counter_write().

## Rights

handle must have ZX_RIGHT_WRITE.

## Return value

zx_counter_write() returns ZX_OK on success.

On failure, an error value is returned.

## Errors

ZX_ERR_WRONG_TYPE  if handle is not a counter handle.

ZX_ERR_ACCESS_DENIED  if handle does not have ZX_RIGHT_WRITE.

ZX_ERR_INVALID_ARGS  if value is an invalid pointer.
```

## Implementation

Counter will be implemented in Zircon by a few CLs (perhaps one).  Starnix
Kernel and Starnix Runner, and the graphics stack will later be updated to use
counter.

The various language bindings, FIDL handle support, fidlcat/fidl_coded,
etc. will be updated.

## Performance

We expect the performance of `zx_counter_create` to be akin to
`zx_event_create`.  We expect the performance of `zx_counter_add` and
`zx_counter_write` to be akin to `zx_object_signal`.

`zx_counter_read` should be similar, except that it may also perform a usercopy
out to return the value.  Whether read performs a usercopy or not is really an
implementation detail.  We can start with an implementation that does perform a
usercopy and later optimize by changing the vDSO/kernel interface to return the
value using a register update the vDSO to store the returned value.

We don't anticipate any significant performance impact (improvement or
regression) for Starnix Suspension use case.

For the Timestamped Graphics Fence use case, we anticipate a small improvement
in both CPU and memory.  Counters will require less memory than VMO based
semaphores (dozens of bytes vs. more than a page) and should be cheaper to
create.

## Ergonomics

Counter is designed to be used with existing async wait patterns (think Zircon
Ports).

## Backwards Compatibility

Counter is a new Zircon object so there are no backwards compatibility concerns.

## Security Considerations

No security considerations.

## Privacy Considerations

No privacy considerations.

## Testing

New core-tests will be added to test the new Zircon object.

## Documentation

Syscall docs will be added/updated.

## Drawbacks, Alternatives, and Unknowns

### Roll your own

As mentioned above, a `zx::event` is not sufficient for the Starnix use case.
However, a `zx::event` plus a shared integer and agreed upon protocol would be
sufficient.  An alternative to introducing a new Zircon object is for Starnix
Runner and Starnix Kernel to use a `zx::event` along with a shared, mapped VMO
containing an integer that they manipulate atomically.  While such a solution
would work, it's more complex and has some subtleties that can be difficult to
get right.  We feel that `zx::counter` is a natual Zircon object and that the
increase in kernel complexity from its addition is minimal.

### General purpose and complete vs. narrow and limited

There is a tension between designing and building a more full-featured counter
or semaphore object, and building one that only satisfies our current use cases.
We've explored providing a similar object in the past, but held off until we had
more concrete use cases.  This time around, we're building a relatively simple
one that is designed to satisfy one or two cases.  Our intention is that as more
use cases arise we'll either evolve or replace this object and not create a
proliferation of similar-but-not-quite-the-same objects.

See also [Future Directions](#Future-Directions).

### Unsigned count

The driving use cases only need an unsigned counter.  We could change the
behavior such that the value is logically unsigned.  We could also use an
options flag to specify whether it's signed or unsigned and make
decrement-below-zero an error when it's unsigned.  Because we have not found
strong arguments to support both signed and unsigned and because signed is more
flexible and can cover more future use cases, the proposal is to provide only
signed semantics.

### Lack of initial value

We considered an alternative where an initial value could be passed to the
create method.  However, callers can accomplish the same thing by simply calling
add before allowing the handle to "escape" so we deemed it unnecessary.

## Future Directions

In the near to medium term we do not anticipate wide spread adoption of
`zx::counter`.  At some point in the future, we may replace `zx::counter` (and
perhaps `zx::event`) with a more general and complete semaphore object.

<!-- Links -->

[go/zx-counter]: https://goto.google.com/zx-counter
[ZirconVmoSemaphore]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/graphics/magma/lib/magma/platform/zircon/zircon_vmo_semaphore.h;l=21;drc=9a0955c33dfbf5eeb1922cbfbfe147f03bfe8ef1
[VkFence]: https://registry.khronos.org/vulkan/specs/latest/man/html/VkFence.html
