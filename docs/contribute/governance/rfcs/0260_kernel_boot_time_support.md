<!-- Generated with `fx rfc` -->
<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0260" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Problem Statement

The [Monotonic Clock Suspension and the Boot Timeline RFC] stated that Fuchsia
must provide a boot timeline that reports the amount of time elapsed since
boot, including any time spent in suspend-to-idle. That RFC mentioned that
there would need to be kernel changes but did not specify exactly what those
were.

## Summary

This RFC describes the syscalls and API changes needed to support the boot
timeline in the Zircon kernel.

## Stakeholders

_Facilitator:_ jamesr@google.com

<TODO>

_Reviewers:_

- adamperry@google.com
- andresoportus@google.com
- cja@google.com
- costan@google.com
- fmil@google.com
- gkalsi@google.com
- harshanv@google.com
- johngro@google.com
- maniscalco@google.com
- mcgrathr@google.com
- miguelfrde@google.com
- surajmalhotra@google.com
- tgales@google.com
- tmandry@google.com

_Socialization:_

This RFC was socialized as a set of several documents that each discussed
various portions of the design. These documents were sent to and iterated on
by the kernel team and by various people across the org.

## Requirements

The kernel **MUST** support:

1. Reading the current value of the boot timeline.
1. Creating custom clocks using the boot timeline as the reference timeline.
1. Creating timers on the boot timeline, though timers that can wake the
system are explicitly left for a future RFC.
1. Reporting the timestamp of log messages and crashlogs in the boot timeline.

Note that we do not plan to support other time related syscalls, such as object
or port waits, on the boot timeline, as we have not identified a motivating use
case to do so. We will revisit this decision in future RFCs if a use case
arises.

## Design

### Introducing New Time Type Aliases in the Zircon API

Zircon has several types for representing time today:

1. `zx_time_t`: Represents a point in time in nanoseconds. Generally this point
is sampled from the monotonic timeline, but not always; `zx_clock_read`, for
example, also returns a `zx_time_t` but is not on the monotonic timeline.
1. `zx_duration_t`: Represents either an amount of time or a distance between
two points in time. This quantity has no concrete association with a timeline,
but is always in units of nanoseconds.
1. `zx_ticks_t`: Represents either a point or a duration of time in platform
specific ticks.

These are all aliases of `int64_t`, which means that the compiler provides no
type checking for any of these entities. They exist solely to improve
the readability of code. Reusing these types for the boot timeline would
significantly degrade the readability of code working with timestamps, as
developers would not be able to easily differentiate between timestamps on
different timelines.

Therefore, new type aliases will be introduced with the following structure:

```
zx_<kind>_<timeline>_[units_]t
```
where:

* `kind` is either `instant` or `duration`.
* `timeline` is either `mono` or `boot`.
* `units` is either `ticks` or omitted, in which case the unit is understood to
be nanoseconds. The vast majority of code uses time in nanoseconds, so this
omission makes the common case a bit easier to type.

Most existing uses of `zx_time_t` will be migrated to use `zx_instant_mono_t`,
and most existing uses of `zx_duration_t` will be migrated to use
`zx_duration_mono_t`. Uses of `zx_ticks_t` will be evaluated and migrated on
a case-by-case basis.

The existing aliases will not be removed, and will be used when an ambiguous
type is required (for example, `zx_clock_read` will still return `zx_time_t`).
It is the aim of the kernel team to remove these ambiguous types in the long
term, but the exact nature of how that happens is out of scope and left to
future RFCs.

### Userspace Types for Time

While the new type aliases for time described in the preceding section will
improve code clarity and readability in the Zircon APIs, they do not provide
compiler-enforced type safety. This type safety will instead be provided by
the userspace syscall wrapper libraries found in the [Rust bindings for Zircon]
and the [C++ libzx library], both of which will need to be extended with the
syscalls introduced in and modified by this RFC. Userspace code should prefer
to use these libraries whenever possible.

Appropriate APIs for strong typing need careful design work in each language
to be idiomatic and easy to use in practice. This won't be rushed and its
completion for each language won't be required for the completion of the type
migration in the C API detailed in the preceding section.

### Reading the Current Boot Time

Two syscalls will be introduced to allow usermode to read the current value of
the boot timeline:

```
zx_instant_boot_t zx_clock_get_boot(void);
zx_instant_boot_ticks_t zx_ticks_get_boot(void);
```

The first will report the time in nanoseconds, and the second will report the
time in platform ticks. This mirrors the corresponding syscalls on the
monotonic timeline.

Note that the ratio of ticks to seconds will be the same for the monotonic and
the boot timelines, meaning that the result of `zx_ticks_per_second` can be
used to convert the results of both `zx_ticks_get` and `zx_ticks_get_boot` to
seconds.

### Creating Custom Clocks

Usermode clocks are created using the `zx_clock_create` syscall. The syscall
already accepts an `options` parameter, meaning we do not need to change the
function signature.

Instead, a new options flag called `ZX_CLOCK_OPT_BOOT` will be introduced that
instructs the newly created clock to use the boot timeline as its reference
timeline.

The timeline property of clocks (monotonic and boot) will be immutable, meaning
that a clock cannot switch timelines once created.

### Renaming Fields in `zx_clock_details`

The `zx_clock_details_v1_t` struct contains two fields that store the
transformation from the monotonic timeline to the clock's synthetic timeline:

```
typedef struct zx_clock_details_v1 {
  // other fields
  zx_clock_transformation_t ticks_to_synthetic;
  zx_clock_transformation_t mono_to_synthetic;
  // other fields
} zx_clock_details_v1_t;
```

These fields will be renamed to highlight the fact that they will now store
the transformation from the _reference_ timeline to the clock's synthetic
timeline:

```
typedef struct zx_clock_details_v1 {
  // other fields
  zx_clock_transformation_t reference_ticks_to_synthetic;
  zx_clock_transformation_t reference_to_synthetic;
  // other fields
} zx_clock_details_v1_t;
```

A quick scan of the code base shows very few users of these fields, and none of
those users are outside of fuchsia.git. Therefore, an in-place rename should be
relatively straightforward.

### Creating Boot Timers

Usermode timers are created using the `zx_timer_create` syscall. This syscall
conveniently accepts a `clock_id` parameter that dictates the timeline the timer
should operate on.

Thus, a new `clock_id` value called `ZX_CLOCK_BOOT` will be introduced that
instructs the newly created timer to operate on the boot timeline.

Similar to clocks, the timeline property of timers will be immutable, meaning
that a timer cannot switch timelines once created.

Note that `zx_timer_set` will be reused to set boot timers. Because this
syscall takes as input both monotonic and boot deadlines, its parameter type
will remain `zx_time_t`.

### Adding the `clock_id` to `zx_info_timer_t`

Users can call `zx_object_get_info` with the `ZX_INFO_TIMER` topic to get
information about a timer handle. The resulting `zx_info_timer_t` struct
will be modified to include the `clock_id` the timer was created with.

Normally, this would require some struct evolution, but in this case the struct
already has 4 bytes of padding, which is just enough space to hold our
`clock_id`, which is stored as a 4 byte `zx_clock_t`.

### Updating timestamps in Zircon structures

There are several timestamps in Zircon structures that report monotonic time
today, but should switch to using boot time once it is available.

Note that timestamps that continue to report monotonic time will not be changed
functionally but will have their types updated to `zx_instant_mono_t` as
described in the type alias section above.

#### `zx_log_record_t` Timestamps

[`zx_log_record_t`] is a struct exposed by Zircon that describes the structure
of a message in the debuglog. This struct contains a `timestamp` field that is
a `zx_time_t` and is a point on `ZX_CLOCK_MONOTONIC`. This will be updated to be
a `zx_instant_boot_t` and therefore will be a point on `ZX_CLOCK_BOOT`.

#### Crashlog Uptime Timestamp

The [crashlog] currently reports an `uptime` field that utilizes monotonic time.
Seeing as monotonic time does not include periods of suspension, this should be
modified to use boot time and its type should be updated to `zx_instant_boot_t`.

### Interrupt API Changes

#### Interrupt Syscalls

The Zircon Interrupt API includes two syscalls that use timestamps:
`zx_interrupt_wait` and `zx_interrupt_trigger`. Currently, these syscalls use
the monotonic timeline. However, because interrupts can fire during
suspend-to-idle, the default timestamp behavior is changing to use the boot
timeline. This helps ensure that interrupt timestamps are unique, even across
suspend-resume cycles, though uniqueness is still limited by clock resolution.
Using the monotonic clock would result in all interrupts that fire during
suspend-to-idle having the same timestamp, which is the time at which the
system suspended.

To maintain compatibility with existing drivers that may rely on monotonic
timestamps, such as the display drivers, a new flag called
`ZX_INTERRUPT_TIMESTAMP_MONO`, is being added to the `zx_interrupt_create`
syscall. When this flag is set, the associated interrupt object will continue
to use monotonic timestamps for `zx_interrupt_wait`, `zx_interrupt_trigger`,
and the interrupt port packet (discussed below).

#### New `zx_object_get_info` Topic

Because interrupt objects will have their timeline set at creation, it's
important that we provide userspace with a way to query the timeline of a given
interrupt handle at runtime. To do this, a new `ZX_INFO_INTERRUPT` topic will
be added to `zx_object_get_info`. Passing in this topic will return the
following struct:
```
typedef struct zx_info_interrupt {
  // The options used to create the interrupt object.
  uint32_t options;
} zx_info_interrupt_t;
```
Callers can then check for the presence of `ZX_INTERRUPT_TIMESTAMP_MONO` in the
`options` field.

#### `libzx` Wrappers

The `libzx` library provides C++ wrappers around the `zx_interrupt_wait` and
`zx_interrupt_trigger` syscalls. These wrappers will need to be updated to use
both monotonic and boot timestamps.

### Port Packet Changes

There are a couple port packets that contain timestamps that will need to be
modified.

#### `zx_packet_interrupt_t`

The `zx_interrupt_bind` syscall allows binding an interrupt object to a port.
When a bound interrupt triggers, a `ZX_PKT_TYPE_INTERRUPT` packet is queued on
the port. This packet includes a `timestamp` field indicating when the
interrupt occurred.

To maintain consistency with the updated interrupt API, the `timestamp` field
in these packets will also transition to using the boot timeline by default.
This ensures that all interrupt timestamps, whether obtained via syscalls or
interrupt packets, provide a coherent view of interrupt timing across
suspend-resume cycles.

However, for interrupts created with the `ZX_INTERRUPT_TIMESTAMP_MONO` flag, the
`timestamp` field in the corresponding packets will continue to use monotonic
timestamps. This preserves compatibility with drivers that depend on this
behavior.

#### `zx_packet_signal_t`

This packet is queued to a port that is bound to an object using the
`zx_object_wait_async` syscall, which has the following signature:

```
zx_status_t zx_object_wait_async(zx_handle_t handle,
                                 zx_handle_t port,
                                 uint64_t key,
                                 zx_signals_t signals,
                                 uint32_t options);
```

When a signal in the set of `signals` is asserted on `handle`, and if the
caller passed `ZX_WAIT_ASYNC_TIMESTAMP` in the `options` field, the resulting
packet is enqueued to the port:

```
typedef struct zx_packet_signal {
  // ... other fields ...
  uint64_t timestamp;
  // ... reserved fields ...
} zx_packet_signal_t;
```

The `timestamp` field records when the signal was asserted. Depending on the
type of object we're waiting on, it would make sense for the timestamp to be on
different timelines. For example, we'd want a boot timestamp when waiting on a
boot timer.

So, we propose the following:

1. Add a new flag to the `zx_object_wait_async` syscall called
  `ZX_WAIT_ASYNC_BOOT_TIMESTAMP`, which signals that the `timestamp` field
  should utilize the boot timeline.
1. Switch the type of `timestamp` to the polymorphic alias `zx_time_t`.

We will audit all existing usages of the `ZX_WAIT_ASYNC_TIMESTAMP` flag and
ensure that any caller that needs boot timestamps is migrated over before we
pause the monotonic clock.

## Implementation

The changes to the kernel API will be implemented first, with each category
of change (reading time, clocks, timers, etc.) separated into different CLs.

These changes (which should be relatively small) will be followed by slightly
larger changes to add boot time support to our Rust and C++ syscall wrapper
libraries. These changes will proceed in two phases:

1. Phase 1 will add support for the syscalls added or modified by this RFC.
This will require some modification of the types used, but this phase will
focus mainly on functional changes.
1. Phase 2 will add more robust type enforcement for time types. This will be
done after Phase 1 as it may take longer to iron out the precise type structure
needed in each language.

## Performance

The syscall modifications needed to support boot time are purely additive,
and therefore should not change any existing performance characteristics of
the system.

Similarly, updating the timestamp field of various structures in Zircon to
use the boot timeline should have no impact on performance, as computing the
boot time is no more expensive than computing the monotonic time.

## Ergonomics

Users of the time syscalls and types may have to be aware of the fact that
there are now multiple timelines supported by the kernel. However, most
programs should be able to just use monotonic time.

## Backwards Compatibility

Except for the semantic change to the timeline of the log record timestamp,
these changes are source and binary backwards compatible.

However, many pieces of existing code may need to switch from using the
monotonic timeline to the boot timeline to preserve correctness. Please refer
to the [Monotonic Clock Suspension and the Boot Timeline RFC] for more
information on how we plan to handle this.

## Security considerations

We do not anticipate any security ramifications from this proposal. For more
information on why this is the case, please refer to the security section of the
[Monotonic Clock Suspension and the Boot Timeline RFC].

## Privacy considerations

This proposal does not impact system privacy, since adding boot time support to
the kernel does not involve any sort of data collection.

## Testing

The [core tests] will be modified to exercise both the new syscalls and the
modifications to the existing syscalls.

## Documentation

All of the Zircon API modifications described above will need to be documented.
Concretely, this means:

1. Documenting all of the new type aliases and updating the docs for the
existing type aliases.
1. Documenting the newly added syscalls.
1. Documenting the new flags added to existing syscalls.
1. Documenting the timeline of the various timestamps that were updated.

In addition, we should update our clock documentation on fuchsia.dev to
highlight how `zx_time_t` is not tethered to a particular timeline.

## Prior art and references

* [Suspend-to-Idle RFC]
* [Monotonic Clock Suspension and the Boot Timeline RFC]

<!-- xrefs -->
[Monotonic Clock Suspension and the Boot Timeline RFC]: /docs/contribute/governance/rfcs/0259_monotonic_clock_suspension_and_the_boot_timeline.md
[Suspend-To-Idle RFC]: /docs/contribute/governance/rfcs/0230_suspend_to_idle.md
[Rust bindings for Zircon]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/lib/zircon/rust/;drc=571b5aa98400e122c8c3db6175ba69df0098c92b
[C++ libzx library]: https://cs.opensource.google/fuchsia/fuchsia/+/main:zircon/system/ulib/zx/include/lib/zx/;drc=4ec7ec271a187aaa9097ec37b3cbc7c1da9ae318
[`zx_log_record_t`]: https://cs.opensource.google/fuchsia/fuchsia/+/main:zircon/system/public/zircon/syscalls/log.h;l=22;drc=a41c2de53392f9b97785d9650561b2ab733f7ee0
[core tests]: https://cs.opensource.google/fuchsia/fuchsia/+/main:zircon/system/utest/core/;drc=d44d7d69ce58c30b3dea36dd77a3d06d1c30a579
[crashlog]: https://cs.opensource.google/fuchsia/fuchsia/+/main:zircon/kernel/lib/crashlog/crashlog.cc;l=127;drc=a662f598032a2ac51d52cc6f423266981ff3bd95