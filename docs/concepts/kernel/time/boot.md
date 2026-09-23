# Boot time

Boot time is a measurement of the time elapsed since the system was last powered
on. It is set to zero whenever the kernel initializes the system's timer
hardware, which happens well before userspace is initialized. Boot time is
guaranteed to progress during periods of Suspend-To-Idle.

Boot time is always available and guaranteed to be continuous and monotonic. It
ticks at a rate determined by the underlying hardware oscillator, and no
attempts are made by Zircon to correct that rate against any external reference.

Components may read boot time using [`zx_clock_get_boot`][clock-get-boot].

[clock-get-boot]: https://fuchsia.dev/reference/syscalls/clock_get_boot