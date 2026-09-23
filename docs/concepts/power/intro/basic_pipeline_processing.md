# Basic Usage

## Preventing system suspension in a data processing pipeline

There are many scenarios where an event requires multiple components
to participate in its processing. Imagine that you are a driver that receives
an event which may wake the system or, if it is received while the system is
awake, should prevent the system from suspending. It is very common that an
interrupt-handling driver may not know the significance of the event and passes
it along to another driver or component.

There are a few options for how to manage this handoff. For the sake of
discussion let's imagine that Driver X receives the interrupt and hands the
event off to Driver Y via a FIDL message. **The options are equally relevant if
X is a driver and Y is a non-driver component.**

### Baton passing

One way to solve this problem is by passing a baton along with the event data.
"Baton passing" alludes to the idea of a relay race where participants pass an
object from one to another to complete the task (ie. race).

Our baton could be [`LeaseToken`][lease_token] (which we talked about
previously) or a [fuchsia.power.broker/LeaseControl][lease_control] channel.
We'll use a `LeaseToken` because we've already talked about them and because
they are easier to work with than `LeaseControl`.

When X receives an interrupt it:

* Calls `ActivityGovernor.AcquireWakeLease`
* ACKs the interrupt
* Does any processing on the event, maybe converting it to a new type
* Passes the event and the `LeaseToken` obtained from `AcquireWakeLease` to Y

This approach requires changes to the protocol used between X and Y. Using
protocol interlocks, described below, does not require this, but has other
downsides.

Systems with high event rates may find the IPC overhead of calling
`ActivityGovernor.AcquireWakeLease` too high to do on _every_ event. In these
cases, instead of simply moving the `LeaseToken` from X to Y, Driver X could
duplicate `LeaseToken` (since it is really just a handle to a Zircon event
pair), pass on the duplicate, and retain the original for some timeout period.
If choosing to use a timeout, **choose your timeout carefully** because the
system will not be able to suspend until all copies of `LeaseToken` are dropped.
Long timeouts are strongly discouraged, the reason to use a timeout is because
you have relatively high event rates, which means you should be able to pick a
short timeout. If you are writing C++ you can use the
[`TimeoutWakeLease`][wake_lease] class for managing leases and extending
timeouts based on the arrival of new events. Use the helper by calling
`AcquireWakeLease` instead of talking to the `ActivityGovernor` capability
yourself.

### Protocol interlocks

"Protocol interlocks" are an option if Driver X and Driver Y's protocol for
event passing contains an acknowledgement or this acknowledgement is added.
The protocol interlock is implemented by Y calling
`ActivityGovernor.AcquireWakeLease` before acknowledging to X that Y received
the event.

The steps for this approach are:

* X calls `ActivityGovernor.AcquireWakeLease`
* ACKs the interrupt
* X does any necessary processing on the event, maybe converting it to a new
  type
* Passes the event to Y
* Y calls `ActivityGovernor.AcquireWakeLease`
* Y acknowledges to X that it received the event
* X drops its `LeaseToken`
* Y does any processing on the event
* Y drops its `LeaseToken`

The protocol interlock does **not** require changing the protocol used between X
and Y if the protocol already has an acknowledgement. This strategy **does**
increase the number of IPC calls to System Activity Governor. Whether or not the
increase in calls is meaningful depends on the rate of events and whether or not
X and Y are retaining their `LeaseToken` for a timeout period. If you choose to
use a timeout, **choose your timeout carefully** because the system will not be
able to suspend until the `LeaseToken` is dropped. Long timeouts are strongly
discouraged, the reason to use a timeout is because you have relatively high
event rates, which means you should be able to pick a short timeout. If you are
writing C++ the [`TimeoutWakeLease`][wake_lease] class is available for managing
leases and extending timeouts based on the arrival of new events. Use the
helper by calling `AcquireWakeLease` instead of talking to the
`ActivityGovernor` capability yourself.

### Timeouts

Using timeouts to ensure system correctness is not the preferred option for
integrating with power framework. The other options use timeouts as an
optimization only. Timeouts can lead to unpredictable behavior because finding a
good timeout value is difficult. Additionally, timeouts usually must be tuned
for each use case, product configuration, and hardware configuration combination
making managing the timeouts a substantial task. Timeouts are inherently racy,
so for any given timeout value it might allow you to win the race most of the
time until it suddenly doesn't. Failures are often discovered only once the
software is deployed to a large test or user population and result in confusion
when observed.

Timeouts are sometimes the only feasible option. With a timeout we'll have
Driver X call `ActivityGovernor.AcquireWakeLease`, ACK the interrupt, and then
hold the lease until we reach the timeout, Driver Y never sees the lease. If
another interrupt arrives before the timeout, Driver X can just extend its
timeout instead of acquiring an additional lease. If you are writing C++ there
is a [helper][wake_lease] class available for managing leases and extending
timeouts based on the arrival of new interrupts. Use the helper by calling
`AcquireWakeLease` instead of talking to the `ActivityGovernor` capability
ourself.

[lease_control]: https://cs.opensource.google/fuchsia/fuchsia/+/39b9a242c6e2b09731a426cdcf9f1353206fd034:sdk/fidl/fuchsia.power.broker/broker.fidl;l=272
[lease_token]: https://cs.opensource.google/fuchsia/fuchsia/+/39b9a242c6e2b09731a426cdcf9f1353206fd034:sdk/fidl/fuchsia.power.system/system.fidl;l=72
[wake_lease]: https://cs.opensource.google/fuchsia/fuchsia/+/39b9a242c6e2b09731a426cdcf9f1353206fd034:sdk/lib/driver/power/cpp/wake-lease.h
