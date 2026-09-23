<!-- Generated with `fx rfc` -->
<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0270" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

## Problem Statement

We need a better way to demultiplex shared interrupts and manage the
acknowledged/unacknowledged state of virtual interrupts.  See Extended Problem
Statement section below for more detail.

## Summary

This RFC proposes adding a new Zircon signal to virtual interrupt objects that
remains asserted when the virtual interrupt object is in the "untriggered"
state.

## Background

### Physical vs. Virtual Interrupts

Zircon interrupt objects come in two flavors, Physical interrupts, and Virtual
interrupts.

Physical interrupt objects logically represent an interrupt signal delivered
through the system's interrupt controller, sometimes via a PCIe bus, and other
times directly to the controller via platform or SoC specific routing.  In
contrast, virtual interrupt objects do not directly represent signals which
originate from non-CPU hardware.  Instead, virtual interrupts are created by
programs in order to look like a physical interrupt, however they can only
become signaled when software calls `zx_interrupt_trigger` on the object.

### Custom waiting interfaces

Interrupt objects do not currently use the standard Zircon signaling system in
order to notify a client that an interrupt has been received.  While they can be
bound to a port and deliver a port packet when an IRQ is received, and they can
be used to block a thread until an IRQ is received, they do not use the standard
`zx_object_wait_one`/`zx_object_wait_many` or `zx_object_wait_async` syscalls
to achieve this behavior.  Instead, users must use `zx_interrupt_bind` to bind
an interrupt object to a port object if they wish to receive port packets, and
must use `zx_interrupt_wait` if they want to block a thread.  Only one logical
wait operation can be pending on an interrupt object at a time, arbitrary
numbers of wait operations are not supported.  Once a thread has unblocked or a
port packet has been delivered, an interrupt needs to be "acknowledged" before
it can become signaled again.

When users are waiting for interrupts via `zx_interrupt_wait`, an interrupt
becomes acknowledged the next time the user's thread blocks on the interrupt by
calling `zx_interrupt_wait` again.  When users bind their interrupt to a port
via `zx_interrupt_bind`, they can acknowledge the interrupt by calling
`zx_interrupt_ack` on the interrupt object, allowing a new port packet to be
delivered.

While this non-standard signaling interface is not the best thing in the world,
and there is some desire to design a new interface which is compatible with the
standard Zircon signaling machinery (in addition to addressing other pain
points), a complete redesign of the Zircon interrupt APIs and transition to a
new system would be a complicated undertaking, likely to take a significant
amount of time and effort and is outside the limited scope of this proposal.

### The Purpose of Virtual Interrupts

Virtual Interrupts serve two primary purposes in Fuchsia, both stemming from the
same motivation.  These are testing, and demultiplexing.

Because drivers need to use the special waiting interfaces used by interrupt
objects, it is difficult to easily substitute a different kernel object (perhaps
an Event) for an Interrupt object and have the driver code "just work".  This
presents an issue when writing driver tests where the test environment wants to
mock hardware.  There is no physical interrupt that can be provided to the
driver, and even if there was, controlling such a thing for any given test is
likely to be difficult and very target specific.

Instead, tests can use a Virtual Interrupt object.  From a driver consumer's
point of view, they use the same API and are basically indistinguishable
allowing the code to be tested without needing any special case code which is
aware of the difference between a testing environment and a real environment.

A similar situation can arise when an individual interrupt needs to be
demultiplexed.  Consider a driver for an external chip which needs to raise an
interrupt.  We'd like to have a single driver for the external chip, independent
of the specific SoC used in a particular product which makes use of the external
chip.  The chip may have an IRQ signal which is connected to the SoC.  This IRQ
signal in some SoC designs might be routed directly to the SoC's interrupt
controller, meaning that a dedicated physical interrupt can be instantiated and
passed to the driver during initialization.  Provided that managing the
interrupt only requires interaction with the system's interrupt controller, the
kernel can directly manage the IRQ via the physical interrupt object.

On the other hand, other designs might connect this interrupt to a GPIO as part
of a block of interrupts (typically in blocks of 16 or 32).  When any of the
GPIOs in the block configured as an interrupt becomes signaled, the physical
interrupt for the GPIO hardware is raised.  The GPIO driver can then wake and
figure out which of the multiple GPIO interrupts has fired, and mask the
interrupt until it can be serviced.  It still needs to signal the driver whose
interrupt this is, however.  We don't want the irq-consumer-driver to need to
know the difference between these situations, nor do we want multiple drivers
for different external chips to need to exist in the same process simply because
the board design routed multiple interrupts to the same GPIO block.  So, in
situations like this, the GPIO driver can create virtual interrupt objects for
each of the GPIOs configured as interrupts in the shared hardware, effectively
demultiplexing multiple interrupts which share the same physical interrupt, and
without requiring the driver to have code to handle the two situations
differently.

## Extended Problem Statement

As of today, however, there is a practical problem with this approach.  Consider
the GPIO demultiplexing scenario.  There are multiple GPIO interrupts sharing
the same block of HW, and the driver has created and distributed one virtual
interrupt for each of them.  When one of the interrupts fires, the GPIO is
signaled via the shared physical interrupt for the block.  The GPIO driver can
then mask the specific bit using the SoC specific GPIO hardware, and
`zx_interrupt_trigger` the associated virtual interrupt.  Now that all of the
pending GPIO interrupts have been signaled, it can acknowledge its own physical
interrupt, and go back to waiting for a new interrupt to be asserted.

Over on the driver/consumer side of things, the interrupt handler is woken, the
interrupt is serviced, and finally acknowledged. At this point, the GPIO driver
needs to take action.  Specifically, it needs to wake up, and unmask/re-arm the
specific bit in the GPIO block before any new interrupt can be signaled.

Currently, however, there is no good way to do this.  No signal is sent via the
interrupt object from the client driver to the GPIO driver, however without such
a thing, the GPIO driver has no good way to know when the interrupt can be
safely unmasked and re-armed.

Additional objects could be introduced, or protocols established, to provide
such a signal, but this would then require the consumer of the interrupt to know
that this is a virtual interrupt instead of a physical interrupt, undermining
the goal of keeping the two scenarios effectively indistinguishable.

## Stakeholders

_Facilitator:_ cpu@google.com

_Reviewers:_
 + 'maniscalco@google.com'
 + 'bradenkell@google.com'
 + 'rudymathu@google.com'
 + 'cpu@google.com'

_Consulted:_

Zircon driver framework team.

_Socialization:_

This proposal was socialized (in Google Doc form) with the Zircon team.

## Requirements

"Downstream" drivers that make use of an interrupt object must be agnostic of
whether it's a virtual one or not.

## Design

### Interrupt objects and standard Zircon signals

As noted before, Zircon interrupt objects do not currently use the standard
Zircon signaling system for the purpose of signaling an interrupt request from
hardware.  This does not mean that they don't participate in the standard
signaling system at all.  Users are free to post async waits to interrupt
objects, or to block threads on them using
`zx_object_wait_one`/`zx_object_wait_many`.  This said, the only way that any of
these wait operations will ever be satisfied is if it was waiting for one of the
8 "user signals" present on practically every Zircon object, and software
asserted that signal.  There are no currently defined "system signals" for
interrupt objects for the kernel to signal.

### ZX_VIRTUAL_INTERRUPT_UNTRIGGERED

So, let's introduce a new signal (call it `ZX_VIRTUAL_INTERRUPT_UNTRIGGERED`),
and use it only for virtual interrupts.  Here are the basic properties of the
signal:

1. Virtual interrupt objects are not initially triggered after creation, so this
signal would be asserted on any newly created virtual interrupt object.
2. When a user calls `zx_interrupt_trigger` on a virtual interrupt object, the
object enters the triggered state and the signal is de-asserted.
3. When a user acknowledges a triggered virtual interrupt object, the object
enters the untriggered state, and the signal is asserted.

### A practical example of usage

Consider a GPIO driver using a physical interrupt `I`, and exposing three
virtual interrupts, `A`, `B`, and `C`, all in the same GPIO block.

1. At startup, the driver creates and distributes `A`, `B`, and `C`, then binds
   `I` to a port `P`.
2. It also creates a dedicated IRQ thread (`T`), configures an appropriate
   profile, and then blocks `T` in `P` waiting for a packet.
3. `I` fires delivering a packet to `P`, unblocking `T` in the process.
4. `T` examines the GPIO state and discovers that the GPIOs associated with `A`
   and `C` are asserted.
5. `T` calls `zx_interrupt_trigger` on `A` and `C`.
6. `A` and `C` enter the triggered state, clearing
   `ZX_VIRTUAL_INTERRUPT_UNTRIGGERED` in the process.
7. `T` calls `zx_object_async_wait` on `A` and `C`, associating them with its
   port and waiting for `ZX_VIRTUAL_INTERRUPT_UNTRIGGERED` to become asserted on
   the virtual interrupt objects.
8. `T` masks the GPIOs associated with `A` and `C`.
9. `T` calls `zx_interrupt_ack` on `I`, then blocks on the port again, waiting
   for a new packet.
10. At some point in time, the driver who consumes `C` wakes up, services the
    interrupt, and then calls `zx_interrupt_ack` on its handle to `C`, asserting
    `ZX_VIRTUAL_INTERRUPT_UNTRIGGERED` as a side effect.
11. The async wait operation waiting for `C`'s untriggered signal is satisfied,
    so a port packet is generated and delivered to `P`, unblocking `T`.
12. `T` wakes, and notices that `C` has become "untriggered", so it unmasks and
    re-arms the GPIO interrupt associated with `C` using its hardware specific
    registers.
13. `T` simply returns to waiting on its port. We will receive a port packet
    when either `I` becomes asserted again, or when `A`'s
    `ZX_VIRTUAL_INTERRUPT_UNTRIGGERED` signal becomes asserted.  No further
    action is required right now.

### Acknowledging a pending interrupt

For some interrupt objects, it is possible for an interrupt to be triggered, and
also have a second IRQ already pending at the time of acknowledgement.  While
this is most typical with MSI interrupts, it is possible to generate this
situation with virtual interrupt objects by simply calling trigger twice in a
row before a client manages to service the interrupt.

When a user acknowledges an interrupt which has another IRQ pending, the result
is that the interrupt object effectively fires immediately.  A thread ack'ing
via `zx_interrupt_wait` will immediately unblock.  A thread ack'ing via
`zx_interrupt_ack` will cause a new port packet to be immediately generated.

What happens with the `UNTRIGGERED` signal?  Since the interrupt logically
became un-triggered, then immediately re-triggered, the behavior of the signal
should be a strobing behavior.  This is to say that any threads currently
waiting for the `UNTRIGGERED` signal should unblock, and any currently posted
async wait operations should be satisfied and produce a port packet.  The signal
state immediately following the acknowledgment operation, however, will be
de-asserted.

## Implementation

Implementation of this approach is expected to be straightforward and low risk.
Kernel code currently has two dispatchers for interrupt objects,
`InterruptDispatcher` and `VirtualInterruptDispatcher`, the second being a
subclass of the first.  While they share a lot of code, the paths for becoming
signaled are separate.  `InterruptDispatcher`s become signaled at hard IRQ time
via the `InterruptHandler` method, while virtual interrupts are signaled at
`zx_interrupt_trigger`ed time via the `Trigger` method.  It is not legal to call
`Trigger` on an `InterruptDispatcher`, doing so will result in a `BAD_STATE`
error.  So, triggering is an operation only performed on virtual interrupt
objects, and always during a syscall context where it is legal to obtain the
locks required to manipulate object signal state.

Acknowledging the interrupt happens either when the next call to
`zx_interrupt_wait` is made (for non-port bound interrupts), or when
`zx_interrupt_ack` is called (for port bound interrupts).  Again, as with
`zx_interrupt_trigger`, the call is made in a syscall context allowing for
signal manipulation.

Finally, interrupts become destroyed either when the last handle to them is
closed, or when a thread calls `zx_interrupt_destroy`.  Again, all of this
happens in a context where kernel mutexes may be obtained, and user signals
asserted, making it simple to assert the untriggered signal (unblocking any
waiters) during explicit destruction of the object.

So, implementation of the feature should consist of simply overloading the
`VirtualInterruptDispatcher`'s behavior during these four operations: trigger,
ack, wait, and destroy.  The overloaded versions may obtain the dispatcher lock
before performing the operation itself, updating the signal state as needed at
the end of the operation.

That's it.  All of the complicated heavy lifting of unblocking threads and
delivering port packets is already taken care of by `UpdateState` and the
existing standard Zircon signaling machinery. Physical interrupt objects will
not expose the signal nor use any of the virtual dispatcher code, sidestepping
the need to synchronize any signal state with operations performed at hard IRQ
time.

## Ergonomics

The ergonomic of this are expected to be simple and acceptable.  _No_ changes
need to be made to existing interrupt consumers, the ack-signaling behavior is
completely automatic.  Virtual interrupt producers simply have to make use of a
signal using existing well established signal handling patterns.

## Security considerations

The addition of this feature effectively introduces a new communications channel
from the interrupt consumer to the interrupt producer, allowing the producer to
know when the consumer has acknowledged a virtual interrupt.  This is part of
the requirements for a system of multiplexed interrupt to function at all, and
is not considered to be a separate or undesirable side channel, and therefore
not a security risk.

## Privacy considerations

No user data or otherwise potentially sensitive information is being handled by
this feature, there are no known privacy consideration which need to be dealt
with.

## Testing

Testing should be a straightforward extension of existing unit tests.  We
basically need to add tests to ensure that:

1. When a virtual interrupt object is created, the
   `ZX_VIRTUAL_INTERRUPT_UNTRIGGERED` signal is initially asserted.
2. The signal is cleared in response to a call to `zx_interrupt_trigger`.
3. The signal is set in response to a call to `zx_interrupt_ack`.
4. The signal properly implements "strobe" behavior when it is acknowledged
   while there exists another pending IRQ.

## Documentation

Documentation for virtual interrupt objects will be updated to describe the new
signal, state that it is only used with virtual interrupt objects, and summarize
the behavior rules stated above (initially asserted, cleared on trigger, set on
ack, strobe on pending ack).

## Drawbacks, alternatives, and unknowns

There are no know significant drawbacks to this approach.  User requirements are
satisfied.  Implementation and testing should be simple, and low risk.

A number of alternative proposals have been discussed by the Zircon driver team,
but all of them possess the same drawback.  Driver consumers of Virtual
Interrupts must become explicitly aware of the fact that they are using a
Virtual interrupt, and then explicitly participate in some user-level defined
protocol to signal acknowledgment to the interrupt producer.

While all of these approaches are _possible_, they seem undesirable.  They add a
significant extra burden on driver authors to participate in whatever protocol
has been defined when a virtual interrupt is being used, and to behave
differently in the case that a physical interrupt is being used.  Additionally,
there is already a pre-existing population of drivers which would need to be
updated to use the new protocol, something which could be skipped entirely if
the kernel handled the ack signal in a transparent fashion.

No matter what protocol gets implemented, it would be implemented entirely in
user-mode, introducing the need to carefully consider trust issues stemming from
potentially malicious actors.  Making the signaling mechanism mediated by the
Zircon kernel helps to simplify the analysis here.

Finally, users of Zircon interrupts _already_ have to acknowledge their
interrupts (either implicitly or explicitly).  It is difficult imagine how any
extra out-of-band ack protocol could end up being as efficient as simply
manipulating the signal bits of a kernel object at the time that the existing
ack'ing takes place.

