# Monotonic time

Monotonic time is a measurement of the time the system has been actively
running since boot. In accordance with this definition, it is set to zero when
the kernel initializes the timer hardware early in boot, and does not include
time spent in Suspend-To-Idle.

Monotonic time is the most reliable time standard on Fuchsia and reading
monotonic time is usually cheaper than reading UTC or local time. Monotonic time
is always available and it always increases continuously and monotonically.
Monotonic time ticks at a rate determined by the underlying hardware
oscillator, and no attempts are made by Zircon to correct that rate against any
external reference.

Since monotonic time counts from power on, it is only meaningful in the context
of a single power cycle on a single Fuchsia device.

Components may read monotonic time using
[`zx_clock_get_monotonic`][clock-get-mono].

[clock-get-mono]: https://fuchsia.dev/reference/syscalls/clock_get_monotonic
