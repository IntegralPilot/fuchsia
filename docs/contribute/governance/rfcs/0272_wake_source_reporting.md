<!--
// LINT.IfChange
-->
<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0272" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Summary

A proposal to extend `zx_system_suspend_enter` to provide user-mode with all of
the specific reasons that the syscall returned, having either just exited the
suspended state, or having never entered it in the first place.

## Background

As a final step of entering low-power states, after having configured platform
specific drivers and components for a low-power state, user-mode may currently
request that the kernel enter its low-power state by calling
`zx_system_suspend_enter`.  The kernel will then take the steps needed to
suspend scheduling and park the CPUs in a lower power state until one of two
things happens; either the user-supplied deadline arrives, or one or more of the
system's "wake sources" becomes signaled.

### Wake Sources

Wake sources are kernel objects which can be used to wake the system from
suspend (or prevent the system from entering suspend) when they are in an
appropriately signaled state.  Currently, the only wake source objects defined
by the Zircon kernel are physical interrupt objects created using
`zx_interrupt_create` while passing the `ZX_INTERRUPT_WAKE_VECTOR` flag in the
options field.  Interrupt wake sources are signaled when the hardware interrupt
associated with the object fires, and remain signaled until they are either
destroyed, or explicitly acknowledged by either posting a new async wait
operation to the object, or by blocking a thread on the object.

### Current behavior of `zx_system_suspend_enter`.

`zx_system_suspend_enter` currently takes 2 parameters.  A resource token, and a
deadline (specified on the boot timeline).  Requesting that the kernel enter
suspend is a privileged operation mediated by access to the resource token,
which must be a system CPU resource.  Failure to pass a valid resource token
will result in an error, and no attempt to enter suspend.

Attempting to call `zx_system_suspend_enter` with a deadline which is already in
the past will return immediately with a status code of `ZX_ERR_TIMED_OUT`.
Attempting to call `zx_system_suspend_enter` with a deadline in the future and
with any wake source objects already in their signaled state will return
immediately with a status code of `ZX_ERR_BAD_STATE`.  Finally attempting to
call `zx_system_suspend_enter` with a deadline in the future and no signaled
wake sources will begin the process of suspending all of the CPUs.  At any point
during or after this process begins, either the arrival of the specified
deadline or the signaling of any wake sources will result in the CPUs being
taken out of suspension, and a returned status code of `ZX_OK`.

### Terminology

+ Wake Source: Any object in the system which, when signaled, can bring the
  system out of suspend, or prevent it from entering in the first place.

+ Deadline Wake Source: A special wake source which exists only in the kernel,
  and can be the reason that suspend was entered when the user supplied deadline
  to a call to `zx_system_suspend_enter` is reached.

+ Wake Source Report Entry: Information about a specific wake source which
  details the activity which may have led to an end of, or failure to enter, the
  suspended state.

+ Wake Source Report: The set of information, including a header and an array of
  Wake Source Report Entries, which may be returned from a call to
  `zx_system_suspend_enter` to inform a user about the causes for exiting, or
  failing to enter, suspend.

+ Wake Report/Wake Report Entry: Shorthand versions of Wake Source Report and
  Wake Source Report Entry.

## Problem Statement

Currently, while the deadline or any wake source can kick the kernel out of the
suspended state, there is no way for user-mode to determine the specific reason
why the suspend operation was completed.

This is problematic, especially in a highly concurrent and asynchronous system,
even if only from a debugging/diagnostics perspective. Given that the Zircon
kernel acts as a central point for all of this logic, and always knows the
precise reason(s) that the system was released from suspend, it seems logical
that it should play a role in assisting user-mode in understanding why
suspension was either blocked or terminated.

## Stakeholders

_Facilitator:_

+ jamesr@google.com

_Reviewers:_

+ eieio@google.com
+ fmil@google.com
+ jmatt@google.com

_Consulted:_

+ maniscalco@google.com
+ surajmalhotra@google.com
+ jaeheon@google.com
+ mcgrathr@google.com

_Socialization:_

An initial doc seeking to clarify requirements was circulated and received
feedback in late 2024.  Later, a more concrete proposal for an API was
circulated and went through several rounds of feedback with various stakeholders
before becoming the basis for this RFC.

## Requirements

Requirements can be partitioned into two types.  Functional requirements (what
user-mode needs) and implementation requirements (rules that kernel code needs
to obey).  While the details of a specific kernel implementation are outside the
scope of this document, it is important to list the kernel requirements to
provide context for some of the design decisions.

### Functional

1) Any time the kernel returns from a call to `zx_system_suspend_enter`, a
   mechanism must be provided which allows callers to know about every wake
   source (including the deadline wake source) which was signaled and either
   prevented the system from entering suspend, or which eventually triggered the
   end of the suspended state.
2) User-mode callers of `zx_system_suspend_enter` must be informed about wake
   sources which had been signaled, but were already acknowledged by the time
   that the reporting mechanism generated the report.  In other words, the fact
   that a wake source became signaled and was acknowledged as the call unwound
   must be reported as well
3) The presence of un-acknowledged wake sources will prevent the system from
   entering into suspend.  Wake source report entries which are waiting to be
   reported for wake sources which are already acknowledged will not prevent the
   system from entering suspend.

### Implementation

1) The memory overhead required for wake report generation must be constant for
   each wake source in the system.  In other words, wake sources which are
   signaled and acknowledged multiple times before being reported must not
   require an unbounded amount of storage.  Instead, the storage requirements
   for a wake source's reporting are logically paid for at the time that a wake
   source is created, and will not grow without bound over time.
2) Generation of a wake report may be serialized using mutual exclusion, and
   take O(N) time where N is the number of wake sources in the system.
3) Generation of a wake report must not hold off interrupt handling or wake
   source signaling for more than O(1) time.
4) The total number of wake sources in a typical system is expected to be in the
   range of 10-20.  It would be considered surprising to encounter a system with
   more than 100.

## Design

### Overview

`zx_system_suspend_enter` will be extended to allow users to pass a pair of
buffers to the kernel which may be used to report wake events to user-mode using
a typical out-parameter pattern.  The kernel will be modified to track wake
sources which are either still in the signaled state, or still waiting to be
reported (or both).  In order to prevent unbound memory requirements, wake
sources will track:

+ When they first became signaled.
+ When they last became signaled.
+ When they last became acknowledged (if ever).
+ A count of the number of times they became signaled.

This will allow wake sources to become signaled and acknowledged an arbitrary
number of times, and still be reported without requiring an arbitrary amount of
memory to store individual signaling events.  The kernel will additionally track
which wake report entries have been reported already, and will indicate this to
user-mode in the case that a wake source becomes signaled, reported, and then
reported once again without first being acknowledged.

### API Changes

Changes to the kernel API consist of the definition of a couple of structures
and flags, and a change to the definition of the `zx_system_suspend_enter`
syscall.

#### Structures and Flags

```C
#define ZX_SYSTEM_SUSPEND_OPTION_DISCARD      ((uint64_t)(1u << 0))
#define ZX_SYSTEM_SUSPEND_OPTION_REPORT_ONLY  ((uint64_t)(1u << 1))

typedef struct zx_wake_source_report_header {
  zx_instant_boot_t report_time;
  zx_instant_boot_t suspend_start_time;
  uint32_t total_wake_sources;
  uint32_t unreported_wake_report_entries;
} zx_wake_source_report_header_t;

#define ZX_SYSTEM_WAKE_REPORT_ENTRY_FLAG_SIGNALED            ((uint32_t)(1u << 0))
#define ZX_SYSTEM_WAKE_REPORT_ENTRY_FLAG_PREVIOUSLY_REPORTED ((uint32_t)(1u << 1))

typedef struct zx_wake_source_report_entry {
  zx_koid_t koid;
  char name[ZX_MAX_NAME_LEN];
  zx_instant_boot_t initial_signal_time;
  zx_instant_boot_t last_signal_time;
  zx_instant_boot_t last_ack_time;
  uint32_t signal_count;
  uint32_t flags;
} zx_wake_source_report_entry_t;
```

#### Syscall Changes

The definition of `zx_system_suspend_enter` will be changed from:

```C
zx_status_t zx_system_suspend_enter(zx_handle_t resource,
                                    zx_instant_boot_t resume_deadline);
```

To

```C
zx_status_t zx_system_suspend_enter(zx_handle_t resource,
                                    zx_instant_boot_t resume_deadline,
                                    uint64_t options,
                                    zx_wake_source_report_header_t* out_header,
                                    zx_wake_source_report_entry_t* out_entries,
                                    uint32_t num_entries,
                                    uint32_t* actual_entries);
```

Additionally, the status code reporting behavior will be altered.  As of today,
`ZX_ERR_TIMED_OUT` is returned when the `resume_deadline` passed to the call is
in the past at the start of the suspend operation, but `ZX_OK` is returned if
the deadline passes while the system is in the process of suspending, or makes
it all of the way into suspend.  Additionally, `ZX_ERR_BAD_STATE` is returned if
there are any signaled wake source at the start of the suspend operation, but
`ZX_OK` is returned if a wake source becomes signaled while the system is in the
process of suspending, or after the system has become fully suspended.

Moving forward, `ZX_OK` will be returned in all of the situations.  A timestamp
(`suspend_start_time`) will be returned in the report header informing the user
of when we actually started to suspend (after all of the parameter checks have
taken place).  Users may detect if a timeout occurred based on the presence or
absence of a wake event for the internal deadline wake source in the report, and
can know if the deadline had already passed by comparing the user-supplied
deadline's value to `suspend_start_time`.  Likewise, users can know whether or
not a wake source was or was not signaled before a suspend operation started by
examining the source's wake report entry's `last_signaled_time` and
`last_ack_time` (when defined) and comparing them to the returned
`suspend_start_time`.

#### Requesting a Report

When requesting that the kernel enter suspend, users *may* request that a wake
report be generated by passing a buffer to contain the wake report header via
the `out_header` parameter.  Users also *may* pass a buffer to contain an array
of wake source report entries to be populated by the kernel, but are *not
required* to; if only a header is desired, only a header buffer needs to be
passed.

Three parameters are currently required to receive report entries:

+ `out_entries` : A pointer to the entry storage.
+ `num_entries` : A number indicating the size of the entry storage.
+ `actual_entries` : An out-parameter pointer to a count of entries actually
  populated in the buffer.

While passing a buffer to contain wake report entries is optional, it is not
legal to pass an inconsistent set of arguments to do so.  In other words, users
must pass a valid pointer for `out_entries` and `actual_entries`, and a non-zero
value for `num_entries`, or they must pass nullptr for both `out_entries` and
`actual_entries`, and zero for `num_entries`.  Finally, while users are allowed
to request only a header (by passing a non-null `out_header`, and `(nullptr, 0,
nullptr)` for `out_entries, num_entries, actual_entries`, it is not legal to
pass valid parameters to hold report entries without also passing a valid
pointer for `out_header`.  Failure to follow these rules will result in a status
code of `ZX_ERR_INVALID_ARGS`, no attempt to enter suspend, and no report being
generated.

When users request a wake report, two counts will be present in the report
header.  First, `total_wake_sources` will report the total number of wake
sources which existed in the system (including the internal deadline wake
source) regardless of their state at the time the report was generated.

When a user passes a buffer to hold reported wake report entries, the report is
limited to reporting at most `num_entries` entries in the user's buffer.  The
actual number reported will be stored in the `actual_entries` out parameter.
Users can know if there are additional events still waiting to be reported (due
to insufficient space in the user's buffer to store the events) by examining the
second counter present in the header, `unreported_wake_report_entries`.

`unreported_wake_report_entries` will report the number of wake report entries
which needed to be reported at the start of report generation, but which did not
fit into the user's buffer and were therefore not reported.  These entries can
be retrieved using a subsequent call to `zx_system_suspend_enter` (see
"Requesting a report without suspending" below).

Note that wake sources can be either created or destroyed after the report is
generated as the call to suspend unwinds, meaning that `total_wake_sources`
might no longer reflect the precise count of system-wide wake sources by the
time that the call returns.  Likewise, wake sources can become signaled (or
re-signaled) during unwind, increasing the number of wake report entries waiting
to be reported beyond what `unreported_wake_report_entries` indicates in the
process.

#### Requesting a report without suspending.

Users may also request that only a report be generated by passing the
`ZX_SYSTEM_SUSPEND_OPTION_REPORT_ONLY` flag in the options.  When this flag is
passed, no attempt to suspend will be made, and the `suspend_start_time`
returned in the header will be `ZX_TIME_INFINITE`.  Additionally, when this flag
is passed, users are (at a minimum) required to pass a buffer to hold a report
header. Failure to do this will result in a `ZX_ERR_INVALID_ARGS` error being
returned.  Also note that the mediating system CPU resource is still required,
even if only requesting a report.

One of the main uses for this flag is to support situations where callers may
have limited or fixed amounts of memory for their event buffer, and the number
of events which need to be reported exceeds the buffer space.  Consider a
situation where a system has space for only 2 events in its buffers.  It calls
suspend, and 10 sources become signaled and acknowledged as the suspend call
unwinds.  The report is generated and 2 of the 10 events are returned to the
user.  In order to drain the remaining events, the user needs to call back into
the suspend operation multiple times.  There are no actively signaled wake
sources to prevent the system from suspending, so the user is forced to pass a
deadline in the past to prevent the system from unintentionally re-suspending.
Every time this happens, the deadline wake source is going to be signaled and
acknowledged, and may or may not end up being re-reported (depending on the
order of enumeration of the events, which is not specified).

In the extreme case where there is only room for one event in the user's buffer,
they may not even be able to make forward progress as the event for the deadline
source may end up being reported over and over again.

#### Report header timestamps

Report headers will contain two timestamps on the boot timeline.  `report_time`
is the time (on the boot timeline) at which the report was actually generated as
the syscall unwinds and just before a return to user mode.  `suspend_start_time`
represents the time at which the syscall commits to the suspend operation.  Wake
events which indicate that a source was last signaled after this time, or which
indicate that a source was last signaled before this time but acknowledged after
this time, are sources which must have had some role in either waking the
system, or preventing it from entering into suspend in the first place.

If, for any reason, the syscall generates a report but makes no attempt to enter
suspend at all, `suspend_start_time` will be `ZX_TIME_INFINITE`.

#### Discarding acknowledged events.

Users who are not interested in receiving wake report entries for sources which
had been signaled but already acknowledged before the call to
`zx_system_suspend_enter` may pass the `ZX_SYSTEM_SUSPEND_OPTION_DISCARD` bit in
the options field when calling `zx_system_suspend_enter`.  At the start of the
suspend operation, any wake report entry for a wake source which has not been
reported, but is currently not signaled (eg; acknowledged) will be discarded as
if it had already been reported.

Note that discarding entries does *not* mark entries for sources which are
currently signaled (whether or not they have been acknowledged previously) as
"having been reported".  The signaled source will prevent the system from
entering suspend, and will be reported to the user (space permitting) even if
the source becomes acknowledged after the attempt to enter suspend, but before
report generation.

It is expected that during the normal course of operation that some wake sources
may be signaled and acknowledged many times before the system makes any attempt
to suspend.  Choosing to discard these events at the start of the operation may
help to reduce the number of events which were not actually instrumental in
waking the system which need to be reported, however doing so is optional.

#### Reported per-wake-event information.

Every wake event reported to users will contain the following fields:

+ `koid` - This is the KOID of the kernel object configured as a wake source
  which became signaled.  As a KOID, it uniquely identifies the specific object
  over the life of the system.
+ `name` - a `ZX_MAX_NAME_LEN` array of characters which contains a
  human-readable string meant to assist with debugging.  The specific format of
  the name is not specified and should never be used by production logic.  It
  exists only for internal development and debugging.  As Zircon interrupt
  objects are not currently re-nameable, the name of the interrupt object will
  be auto-generated.
+ `initial_signal_time` - The time, on the boot timeline, at which this wake
  source first became signaled.  If the source is acknowledged and re-signaled
  multiple times before being reported, `initial_signal_time` will remain the
  same, recording the time of the first signaling event.
+ `last_signal_time` - The most recent time, on the boot timeline, at which this
  wake source was signaled before being reported.  If the source is acknowledged
  and re-signaled multiple times before being reported, `last_signal_time` will
  be updated each time, always recording the most recent time at which the wake
  source was signaled.
+ `last_ack_time` - The most recent time, on the boot timeline, at which this
  wake source was acknowledged.  If the source was never acknowledged before
  being reported this will have the value `ZX_TIME_INFINITE`.  In the case that
  a wake source is signaled and acknowledged multiple times, then signaled once
  more before being reported, it is possible for the `last_ack_time` to be less
  than the `last_signal_time`, however it will always be greater than or equal
  to the `initial_signal_time`.
+ `signal_count` - Contains the number of times that a wake source became
  signaled between the initial and last signal times.  `signal_count` will
  always be at least one, and in the case that `signal_count` is exactly one,
  the initial and last signal times will be the same value.
+ `flags` - Contains a combination of two currently defined flags.

    - `ZX_SYSTEM_WAKE_REPORT_ENTRY_FLAG_SIGNALED` - Indicates that the wake
      source for this wake report entry was still signaled (has not yet been
      acknowledged) at the time that the wake report containing this entry was
      generated.
    - `ZX_SYSTEM_WAKE_REPORT_ENTRY_FLAG_PREVIOUSLY_REPORTED` - Indicates that
      this wake report entry has been present in a previous report.  Please
      note, it is not possible to see an entry which had been previously
      reported, but is not currently signaled.  To join the set of wake report
      entries waiting to be reported the wake signaled must be signaled, and as
      soon as a wake source for a wake report entry becomes both acknowledged
      (clearing the signaled bit) and reported (setting the reported bit), it is
      removed from the set.

#### Reporting the Deadline Wake Source.

When a call to `zx_system_suspend_enter` is made with a `resume_deadline` in the
future, and the system enters suspend only to be woken by the deadline timer,
this will be reported to the user in the form of a special wake report entry.
This entry's `koid` field will contain the value `ZX_KOID_KERNEL`. This wake
source is automatically acknowledged by the kernel; no special user-mode actions
need to be taken to manage the deadline wake source.

#### Destruction of wake sources.

When a wake source created by user-mode is destroyed, it is implicitly
acknowledged so that it cannot prevent the system from entering suspend.
Additionally, if there is a wake report entry waiting to be reported for the
destroyed wake source, it will immediately be removed from the set of wake
report entries waiting to be reported.


## Testing

Testing of wake report generation will be done using unit tests in a core-test
environment.  These unit tests can only be run reliably as core-tests since the
presence of platform specific wake source in the form of physical interrupts
could otherwise make the results of report generation non-deterministic.

In addition to being run in a core-test environment, we will need to add one
more thing to facilitate testing; non-platform specific wake sources which can
be both signaled and acknowledged by test code.  Physical interrupts do not
satisfy these requirements.  The presence of a physical interrupt in a system is
platform specific detail, we cannot rely on any specific set of them existing
for an arbitrary platform.  Additionally, physical interrupt objects are not
things which can be force-triggered by user-mode.  They can only be triggered by
their associated hardware.

Since physical interrupts cannot be used, and currently there are no other types
of wake sources defined in the system (aside from the special case of the
deadline wake source) we plan to introduce a new type of wake source
specifically intended for testing.

### Virtual Interrupt Wake Sources.

In addition to physical interrupt objects, Zircon also supports "virtual"
interrupt objects.  Virtual interrupt objects participate in the same special
case signaling that physical interrupts do, and are generally used in situations
where blocks of interrupts need to be de-multiplexed in order to maintain
process isolation when one driver owns the block of interrupts, and several
other otherwise unrelated drivers use interrupts in the block.  Blocks of GPIOs
configured as interrupts are the most common occurrence of this today, however
it does not have to be the only one (for example, USB interrupts could also
potentially make use of these objects).

Unlike physical interrupts, virtual interrupts can only be triggered
programmatically by user-mode code (by calling `zx_interrupt_trigger`).  They
are created using the same syscall as physical interrupts
(`zx_interrupt_create`), however they do not currently require a valid handle to
the system IRQ resource to create (physical interrupts do) and they are
explicitly prevented from being created as wake sources.

Moving forward, we will alter these restrictions.  Specifically, users will be
allowed to create virtual interrupts objects as wake sources, but only if they
provide a valid handle to the system IRQ resource.  In other words, creation of
a virtual interrupt wake source will be considered to be a privileged operation
mediated by the same system resource which controls the ability to create
physical interrupt objects.  Non-wake-source virtual interrupts will continue to
be constructable without any special permissions.

This will allow a complete set of deterministic unit tests to be written and run
in a core-test environment

## Performance

Performance impact is expected to be minimal.  Interrupts which result in a wake
source becoming signaled will need to contend with a lock in the rare case that
a report is being generated at the same time that the wake source is becoming
signaled, however report generation will never hold the lock for O(N) time, only
for long enough to evaluate the state of an individual associated wake report
entry, and to copy the contents to a local buffer before dropping the lock and
attempting to copy to the user supplied buffer.

Report generation itself will take (as a matter of necessity) O(N) time, during
which new wake sources cannot be created or destroyed in the system.  This is
not anticipated to cause any meaningful performance problems as it is expected
that the vast majority of wake sources will be created during system
initialization, and will exist for the lifetime of the system.

## Backwards Compatibility

This approach directly modifies an existing syscall, and is therefore not
backwards compatible from an ABI perspective.  That said, the syscall being
modified is currently a member of `@next` and is still under development,
meaning that there should be no external dependencies which need to be handled
using an explicit deprecation cycle.

Existing call sites in the code base may be modified to pass `0, nullptr,
nullptr, 0, nullptr` as defaults for the call, which (except for changes to the
returned status code behavior) will retain the current behavior of the system.
Wake report entries will accumulate internally, but at no significant cost.

## Security considerations

The are no significant security concerns anticipated.  The introduction of
virtual wake source interrupts is mediated by the same resource token that
physical wake source interrupt creation is mediated by.  Additionally, all calls
to `zx_system_suspend_enter` are already mediated by access to a system cpu
token, automatically making the ability to generate a wake source report
mediated by the same.

## Privacy considerations

There is no PII involved in either suspending the system, or reporting the
reasons that the system came out of suspend, therefore there are no known
privacy concerns.  In the case that knowledge of the trigger and acknowledgement
times of wakes sources are considered "private" information, users can rest easy
knowing that the ability to generate a report is privileged operation mediated
by a resource token.

## Documentation

Currently, `zx_system_suspend_enter` is a API in development and exists under
`@next`.  As such, no formal public documentation exists.  In the short term,
internal users of wake source reporting will be able to use internal design
documents, this RFC, and unit tests as a reference for specified behavior and
usage examples.

After Zircon's support for entering/exiting suspend and reporting specific wake
reasons has stabilized, formal public documentation for the sub-system's final
form will be written.  Assuming that this mechanism for wake source reporting
still exists at that point in time, it is anticipated documentation will be
written reflecting the rules spelled out in this RFC, in addition to any changes
made in the intervening time.

## Alternatives Considered

### Timer Wake Sources

In addition to interrupts, there could be utility in being able to create Zircon
Timer objects which were also configured to be wake sources.  Users could
schedule deadlines on the boot timeline, and when their timer object becomes
signaled, the wake source would be considered to be signaled and remain so until
the timer was acknowledged by either being cancelled, or re-scheduled in the
future.

For now, we have decided not to do this for a few reasons.
1) Wake sources are powerful objects with the potential to prevent the system
   from entering a suspended state, and therefore possess much potential for
   abuse. Currently, unlike interrupts, there is no explicit capability which
   needs to be provided in order to create a timer meaning that there is no
   immediately available way to treat the construction of a timer wake source as
   a privileged operation mediated by a capability in the form of a handle.
2) The `zx_system_suspend_enter` call already supports a deadline.  Should
   user-mode have a need to support an arbitrary number of timers which can
   bring the system out of suspend, a simple timer queue can be implemented at
   the level of the component who possesses the capability to call
   `zx_system_suspend_enter`. This also puts control of any required capability
   model entirely in the hands of the power management subsystem in user-mode.
3) Finally, we can always come back and do this later.  A new RFC would need to
   be written to address the issues of how we mediate the creation of wake
   source timers, but the wake report entry reporting structure present here
   should be sufficient to report wake sources triggered by timers just as well
   as it does for interrupts.

### Union or TLV-based wake report entries.

The proposed structure for wake report entries is currently fixed.  In a world
where there are many different types of wake sources in the system, each with
highly differentiates sets of important details, it is possible to imagine a
world where there are non-overlapping sets of information which need to be
reported for the different types of wake sources in the system.

In a world like this, what would a wake report entry object look like?  One
possibility would be to define the structure as having a type enumeration and a
union of the various non-overlapping sets of data.  Basically, a C ABI version
of a `std::variant`.  While this is possible, it would be rather awkward.  Right
now there is only one type of user-constructible wake source in the system, so
no need for a union.

If we wanted to prepare for a future (unknown) wake source type, we'd need to
pad the union out to a maximum possible size (also unknown) in order to
hopefully keep the union constant size when we finally do add a new wake report
entry type.  But when that day arrives, we may not have reserved enough space
and be forced to rev the interface anyway, and until that day arrives, the
reserved space would simply be wasted.

Similarly, we could adopt a Type-Length-Value (TLV) approach to encoding wake
report entries.  This approach would not require making guesses as to the
"maximum possible size of an encoded wake event", however it would introduce a
kernel ABI which used sources of variable length objects, complicating both the
kernel code needed to serialize the report, and the user-mode code required to
process it.

At the end of the day, the current fixed wake report entry definition is
adequate for both the existing use case (interrupt wake report entries), as well
as the only identified potential future use case (timer wake sources), so there
is no compelling reason to complicated things by adopting a more complicated
approach right now.

### Additional user-mode info.

Depending on how user-mode plans to make use of reported wake report entries, it
might be advantageous to permit user-mode to associate a binary blob (of fixed
maximum size) with a wake source, which would then be reported in wake report
entries during report generation.  This would allow user-mode to "tunnel"
arbitrary bookkeeping through wake source objects and into the reported wake
report entries.

While potentially useful, the existing customers for the wake reporting feature
have indicated that they will not need such a thing.  The reported KOID is
sufficient; it can act as a unique key which can be used to look up user-mode
bookkeeping information if necessary.

Not doing this also means that we do not have to worry about designing things
like the capability model for controlling this information, nor do we need to
worry about the potential security risks associated with a malicious actor
gaining access to the private bookkeeping information via wake source reporting.

### Reporting wake events for destroyed wake sources.

As noted earlier, when a wake source is destroyed, any associated wake report
entry information waiting to be reported is immediately destroyed and will never
be reported.  Should we instead insist on reporting wake report entries for wake
sources which have been destroyed already instead?

While the proposed behavior means that it is technically possible to fail to
report a wake report entry to user-mode, this is not anticipated to pose any
serious problems in typical use case scenarios.  As of today, wake sources are
always interrupts which tend to be allocated early during system startup and
live for the lifetime of the entire system, meaning that they will not be
destroyed during normal operation.  Making the choice to immediately remove any
wake report entries waiting to be reported is a simple way to ensure that the
memory requirements for wake reporting are not able to grow without bound if
something in the system starts to:

Create a wake source
+ Signal it.
+ Destroy it.
+ Repeat forever.

Moving forward, if new types of wake sources are introduced which are routinely
created and destroyed during regular operations, and the potential to lose wake
report entry information for destroyed sources is causing problems, this
decision can be re-visited.

<!--
// LINT.ThenChange(//tools/rfc/test_data/rfc.golden.md)
-->
