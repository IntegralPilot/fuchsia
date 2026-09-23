# UTC behavior

This page outlines the behavior of the UTC clock on Fuchsia. See the
[UTC overview](overview.md) for more information on UTC time, and the
[time overview](/docs/concepts/kernel/time/overview.md) for information on the other
clocks available on Fuchsia.

UTC time is supplied by a
[kernel clock object](/docs/reference/kernel_objects/clock.md) that is
distributed to components. A process cannot read UTC time unless it is passed a
handle to the clock when launched. All components started by Component Manager
receive a handle to the clock.

[Timekeeper](/src/sys/time/timekeeper) sets and maintains the UTC clock by
synchronizing against either a Real Time Clock (RTC) or an external source
generally accessible over the network.

## Differences from other operating systems

UTC time on Fuchsia differs from time on most other operating systems in that
it enforces a backstop time. The backstop time is set to the time of the latest
commit included in a build, and serves as a "well known" time. Fuchsia will
never report a time earlier than the backstop time, even if some time source
reports an earlier time. This backstop is included to limit attacks where an
adversary manipulates time on the device. As an example, such an attack could
be used to force the device to accept an expired TLS certificate.

Before UTC time is first synchronized, the only estimate of UTC time available
to Fuchsia is the backstop time. Backstop time by itself is not a reliable
estimate of time as any amount of time may have passed between the time of the
commit and the time the device is booted. To communicate this uncertainty,
prior to the first time synchronization the UTC clock may not run, and if it
runs it is started from the fixed backstop time. Whether the UTC clock may run
when not synchronized is a parameter that each Fuchsia product must set based
on the specific product's requirements.

In summary, the UTC clock on Fuchsia has the following states. The UTC clock
starts in a fixed state and ends in some running state.

State         | Description | Clock behavior
--------------|-------------|---------------
Fixed | Time has never been synchronized and is unreliable | Time is fixed at backstop.
Running, not synchronized | Time has not been synchronized | Time is running, starting from the backstop. In this state the UTC clock on Fuchsia behaves similar to clocks on other OSs. On entry into this state `ZX_CLOCK_STARTED` signal is asserted.
Running, synchronized | Time has been synchronized at least once | Time is running. In this state the UTC clock reading is tracking an external time source. On entry into this state, `ZX_USER_SIGNAL_0` is asserted.

## Properties

The UTC clock always exhibits the following properties:

* Backstop time - the clock is created with a backstop time set to the time of
the last commit in the build. The clock will never report a time earlier than
the backstop.
* The UTC clock is neither monotonic nor continuous. As UTC time must be
synchronized from an external source, Timekeeper may jump the time backwards if
it finds that its estimate of UTC time has drifted far ahead of the external
source.

Before Timekeeper first synchronizes time, the best estimate of UTC time
available on the device is the backstop time. Therefore, it is important to
note that during this state the UTC time may be reported with an arbitrarily
large error.

However, the clock *may* be running regardless of whether its reading corresponds
to actual UTC timestamp. See the clock behavior above to determine the UTC clock
state by observing appropriate events on the UTC clock handle.

After Timekeeper synchronizes the time, it sets the UTC clock. This may cause
the clock reading to jump abruptly. From this point on, Timekeeper will
continue to update the clock by adjusting the clock frequency to run slightly
faster or slower, or by jumping the clock to a new time. While accuracy varies
between products and time synchronization methods, the clock is generally
within a few hundred milliseconds of the actual UTC time once running. Note
that even when an RTC, network, or both are available, the UTC clock may never
be synchronized as it must be retrieved over fallible protocols from fallible
sources.

## Observable behaviors

As a result of the properties above, you may observe the following behaviors:

* UTC time may run up to a few hundred parts per million (ppm) faster or slower
than monotonic time. This occurs as Timekeeper slews the clock slower or faster
to compensate for oscillator errors or to correct small errors.
* UTC time may jump forwards or backwards an unbounded duration. This occurs if
Timekeeper needs to correct for a large error. A large jump forward is expected
when time is first synchronized. Subsequent jumps forwards and backwards should
be very rare and are usually caused by errors in the time source.
* UTC time may not be running. This may occur prior to the first time
synchronization.

## Strategies for handling the unsynchronized state

Components launched soon after a device boots, or that need to run before any
network is available should expect to encounter situations where the UTC clock
is not yet synchronized with UTC actual. In the rare case time synchronization
never succeeds, components launched later will also see an unsynchronized
clock. Some example strategies are:

* Ignore the unsynchronized state and read UTC time.
This strategy is appropriate for cases where the UTC time doesn't strictly
need to be accurate, such as generating timestamps for debug purposes. It is
also appropriate for cases where it is known a component will not be run until
time is synchronized. The downside of this strategy is that you may see
consecutive timestamps that all report the same time.

* Wait for UTC time to synchronize before reading the clock.
This strategy is appropriate for cases where UTC time accuracy is critical,
such as using UTC time to validate credentials. Note that this is not always
the preferred strategy as UTC time may never synchronize.

## Checking clock properties

You may check clock properties by first obtaining a handle to the zircon clock
object, then passing the handle to the appropriate syscalls. As an example,
this is useful if you need to check that time is synchronized before reading
the clock. This is particularly important for applications such as TLS
certificate validation as some reasonably accurate time is needed to verify
expiry dates.

A handle to the UTC clock provided to the runtime is retrievable using
the `zx_utc_reference_get` method provided in
[`<zircon/utc.h>`](/sdk/lib/c/include/zircon/utc.h).

The
[`ZX_CLOCK_STARTED`](/docs/reference/kernel_objects/clock.md#starting-a-clock)
signal is asserted when the clock is running. The
`SIGNAL_UTC_CLOCK_SYNCHRONIZED`, (or equivalently `ZX_SIGNAL_USER_0`) is
asserted when the clock is first synchronized.

You may check or wait for either of the signals using one of:

* [`zx_object_wait_async`](/reference/syscalls/object_wait_async.md)
* [`zx_object_wait_many`](/reference/syscalls/object_wait_many.md)
* [`zx_object_wait_one`](/reference/syscalls/object_wait_one.md)

You may check details such as the clock's error bound using
[`zx_clock_get_details`](/reference/syscalls/clock_get_details.md).
For the UTC clock the error bound is defined as half of a 95% confidence
interval. In other words, for a randomly selected time on a randomly selected
Fuchsia device, there is a ≥95% probability that the true value of UTC is
between `reported_utc - error_bound` and `reported_utc + error_bound`. Until
the system has an estimate of `error_bound`, the `error_bound` is set to
`ZX_CLOCK_UNKNOWN_ERROR`. In some cases, if a Fuchsia device is running
abnormal workloads or has defective hardware, the true UTC time may fall
outside the range defined by `error_bound`. See the
[How can clock error be bounded?](algorithms.md#error_bound) section for
details on how the error bound is calculated. If you require additional details
about the UTC clock, see the
[kernel clock reference](/docs/reference/kernel_objects/clock.md)
for a list of details provided through `zx_clock_get_details`.

Note that components are provided a read-only handle and are unable to use the
provided handle to modify the clock.

For language specific bindings and examples, see
[language support](/docs/concepts/kernel/time/language_support.md).
