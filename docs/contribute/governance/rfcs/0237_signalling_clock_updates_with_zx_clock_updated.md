<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0237" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Summary

This RFC proposes introducing a new clock signal `ZX_CLOCK_UPDATED` to Zircon
clocks, which strobes each time the clock updates.

## Motivation

A [clock](/reference/kernel_objects/clock.md) is a
[one dimensional affine transformation](/docs/concepts/kernel/clock_transformations.md)
of the
[clock monotonic](/reference/syscalls/clock_get_monotonic.md) reference
timeline, defining how the "reference" time (i.e. the device's monotonic clock)
should be translated to the "synthetic" time output by the clock. Thus, a [UTC](/docs/concepts/kernel/time/utc/overview.md)
clock is a constantly adjusting transform which, when applied to the current
value of the device's monotonic clock, produces the current UTC time.

The affine transform of the UTC clock in Zircon is updated by Timekeeper only
using the syscall `zx_clock_update` and can be read by any clock user using
the syscall `zx_clock_get_details`.

In order for a Linux program running on Starnix to calculate UTC time using
the vDSO function `clock_gettime` or `gettimeofday`, the Starnix vDSO must have
access to the most up to date clock transform from monotonic to UTC time.
Currently the Starnix kernel is able to provide this access by regularly
polling the Zircon UTC clock for the clock transform, using
`zx_clock_get_details`, then loading this transform into a page in memory that
is shared by the Starnix kernel and the userspace of Linux programs running on
Starnix.

A strobe is an infinitely fast assertion and deassertion of a signal. This
means that observers waiting for a change to the clock signal can detect a
strobe, but no observer will ever read the value of the clock signal to be
asserted. By introducing `ZX_CLOCK_UPDATED`, a clock signal which strobes
when the clock updates, the Starnix kernel can instead be notified of updates
to the UTC clock, and get the most up to date transform from Zircon then update
the clock transform used by Linux programs, immediately after these
notifications. This would reduce the potential latency between the clock
transform changing in Zircon and this new transform being used when Linux
programs calculate UTC time.

## Stakeholders

Who has a stake in whether this RFC is accepted? (This section is optional but
encouraged.)

*Facilitator:*

-   cpu@google.com

*Reviewers:*

-   abarth@google.com
-   maniscalco@google.com
-   johngro@google.com

*Consulted:*

-   mariagl@google.com
-   fdurso@google.com
-   qsr@google.com

*Socialization:*

Discussed with all reviewers and consultants listed above. A
[design doc](https://docs.google.com/document/d/1-P5XSHMZdo4CO3DQmS795peJnKJdeGvPDVu3k_X5V0U/edit#heading=h.b2iwbvv6cjzw)
preceded this document and received informal feedback from all stakeholders.

Information from this document has been incorporated into this RFC,
where relevant.

## Design

Zircon clocks contain one signal: `ZX_CLOCK_STARTED` which is asserted when the
clock starts. The design adds a new clock signal, `ZX_CLOCK_UPDATED` which is
strobed each time the clock is updated.

In order to be able to strobe a signal, the design also involves the
modification of `Dispatcher::UpdateState` which is currently used to update
Zircon signals. Currently, the function takes two inputs, a `set_mask` and a
`clear_mask`, which specify the signals to set (assert) and clear (deassert).
Alongside updating the signals specified by the `set_mask` and `clear_mask`,
the function notifies all observers of any signals which have transitioned
from inactive to active, using the function `NotifyObservers`.

The modification is to add a third input, defaulted to zero, to `UpdateState`,
named `strobe_mask`. This specifies all of the signals to strobe. Signals which
are specified to strobe do not change value, but the `NotifyObservers` call is
modified to also notify all observers of any signals specified by
`strobe_mask`, so that the observers are informed that there has been an update
to the signal.

In order to use this signal to be informed of updates to the clock transform in
a way which will prevent the user from missing updates, users should not use
[`zx_object_wait_one`](/reference/syscalls/object_wait_one.md)
or [`zx_object_wait_many`](/reference/syscalls/object_wait_many.md)
to wait for a signal strobe. If a user reads the current clock details, then
calls `zx_object_wait_one` to wait for an update to the clock, the clock may
have been updated by Zircon in between the reading of the current clock details
and the call of the wait, and so this update will not be registered.

Instead, [`zx_object_wait_async`](/reference/syscalls/object_wait_many.md)
should be used. Before reading the current transform, the user should post an
async wait against the clock. Then, after reading the clock details, the user
can wait on the port that the async wait was posted to. This prevents an update
to clock details being missed.

A result of this mechanism is that if the clock is updated in between the async
wait being created and the clock details being read, this leads to an immediate
wakeup when waiting on the port, despite the fact that the clock transform that
was read is up to date. Future work could be to evolve this signal to remove
the possibility of these spurious wakeups.

## Implementation

This design can be implemented in one small CL. The CL defines the new signal
and modifies UpdateState to include a new `strobe_mask` input.

## Performance

This new signal will be used by Starnix kernel to reduce the potential error of
the UTC clock of Linux programs running on Starnix.

At the moment, Starnix is the only proposed user of this signal, however there
may be other users of this signal in the future (for example, implementations
of the C++ runtime in order to support the C++ standard library's ability to
block threads until a UTC deadline). The time complexity of strobing a signal
using `UpdateState` is linear with respect to the number of observers of the
signal. This means that when the number of observers of `ZX_CLOCK_UPDATED`
becomes large, there will be performance issues.

## Backwards Compatibility

There is no change to the behavior of any existing signals, so no backwards
compatibility issues are anticipated.

## Security considerations

No security ramifications from this proposal are anticipated.

## Privacy considerations

No privacy ramifications from this proposal are anticipated.

## Testing

Tests will be added which verify the behaviors specified in the design. The
tests will ensure that no users of the clock can change the signal manually,
that any observers waiting for a strobe on a clock's `ZX_CLOCK_UPDATED` signal
are notified when the clock updates, and that observers of the signal are not
notified when there has been no clock update (meaning ZX_CLOCK_UPDATED is never
asserted when there is no clock update).

## Documentation

The following docs will be updated to describe `ZX_CLOCK_UPDATED`, and the
method clock users should follow to not miss an update when using this signal.

- [/docs/concepts/kernel/time/utc/architecture.md](/docs/concepts/kernel/time/utc/architecture.md)
- [/docs/concepts/kernel/time/utc/behavior.md](/docs/concepts/kernel/time/utc/behavior.md)
- [/reference/kernel_objects/clock.md](/reference/kernel_objects/clock.md)
- [/reference/syscalls/clock_update.md](/reference/syscalls/clock_update.md)

## Drawbacks, and alternatives

### Drawbacks

The issue of performance decreasing as the number of observers of the signal
increases is described in the [Performance](#Performance) section of this
document. If the number of observers of the clock signal increases to a large
number, further work may need to be done to mitigate these performance issues.

### Alternatives

An alternative solution to the problem posed by the motivation is to map the
clock details of the Zircon UTC clock directly into Starnix kernel.
The Starnix kernel then can map this memory directly into the Linux processes'
space. This means that any updates to the clock object in Zircon are
immediately propagated to Starnix and to the Linux processes. This would remove
the need to introduce a new signal, and instead of reducing the latency of
clock updates being propagated to Starnix, it would remove this latency
entirely. However, this would require a sizable change to the Zircon kernel.
