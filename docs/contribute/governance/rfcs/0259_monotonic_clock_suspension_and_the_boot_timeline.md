<!-- Generated with `fx rfc` -->
<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0259" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Problem Statement

The [Suspend-To-Idle RFC] stated that the monotonic clock should be paused
during periods of suspend-to-idle. However, there are programs running on
Fuchsia that may need to be aware of time elapsed during periods of
suspension. These programs will break if we pause the clock without providing
an alternative way for these programs to track time.

## Summary

This RFC provides a solution to the problem stated above by proposing the
addition of a new timeline, henceforth referred to as the "boot timeline," that
will continue to progress during periods of suspend-to-idle. We will also
introduce clocks, timers, and other related objects that interact with this
timeline.

Existing code that needs to be aware of time elapsed during periods of
suspension will need to be modified to use the boot timeline before we make the
change to pause the monotonic clock.

## Stakeholders

_Facilitator:_ jamesr@google.com

_Reviewers:_

- adamperry@google.com
- alizhang@google.com
- brunodalbo@google.com
- crjohns@google.com
- fmil@google.com
- gmtr@google.com
- johngro@google.com
- maniscalco@google.com
- miguelfrde@google.com
- tgales@google.com
- tmandry@google.com

_Consulted:_

Many teams within Fuchsia were consulted for the contents of this RFC. This
includes, but is not limited to, folks on the following teams:

* Forensics
* Diagnostics
* Kernel
* Performance
* Security
* Starnix
* Timekeeping

_Socialization:_

The contents of this RFC were socialized in several separate documents that were
sent to various mailing lists that encompass the majority of the Fuchsia
organization.

## Requirements

This RFC places several requirements on both the existing monotonic timeline and
the new boot timeline:

* Introducing the boot timeline and pausing the monotonic clock during periods
of suspension **MUST NOT** prevent applications from implementing their
existing functionality.
* The boot clock **MUST** report the total time elapsed since system boot,
including any time spent in suspend-to-idle.
* The monotonic clock **MUST NOT** include the time spent in suspend-to-idle.
Prior to the [Suspend-To-Idle RFC], the behavior of the monotonic clock during
system suspension was undefined, so we're defining that behavior explicitly
here.
* Both the monotonic and boot clock **MUST** be monotonically increasing.
* Both the monotonic and boot clock **MUST** be set to zero when the kernel
initializes the timer hardware.
  * Timer hardware initialization, in turn, **MUST** happen before userspace
  starts.

## Design

This proposal describes the new boot timeline's behavior as well as changes in
the monotonic timeline's behavior. It also provides an overview of the portions
of Fuchsia that may need to use the boot timeline to accommodate a pausing
monotonic clock. More detailed descriptions of the API changes needed will
follow in future RFCs.

### Naming Considerations

While every major operating system has a timeline that does not pause when the
system suspends, each one has a different name for that timeline. Here is a
table showing the two timelines we have in Fuchsia and what they're called in
other operating systems:

| Fuchsia | Linux | BSD | Windows | OS X | Android |
| ------- | ----- | --- | ------- | ---- | ------- |
| Monotonic Time | CLOCK_MONOTONIC_RAW | CLOCK_UPTIME | Unbiased Time | Absolute Time | Uptime |
| Boot Time | CLOCK_BOOTTIME | CLOCK_BOOTTIME | Realtime or Time | Continuous Time | ElapsedRealtime |

The name "boot timeline" was chosen to match the existing names used by Linux
and BSD, especially because POSIX compatibility (via Starnix) is one of our
primary reasons for introducing this new timeline to the Zircon kernel.

### Timeline Semantics

The boot timeline is guaranteed to *progress* during periods of suspend-to-idle.
In contrast, as described by RFC-230, the monotonic timeline is guaranteed to
*pause* during periods of suspension. However, both timelines will tick at the
same rate when the system is not suspended. Both timelines will also be set to
zero when the kernel first initializes the timer hardware, which currently takes
place early in kernel startup.

### Affected Parts of Fuchsia

The following parts of Fuchsia will need to change to accommodate the pausing
monotonic clock. The goal of this RFC is to describe the areas that need to
change at a high level. Future RFCs and design documents will clarify exactly
what those changes are and how they will be made.

Note that each area listed here should also audit the FIDL services they provide
to ensure that any timestamp fields document the timeline they operate on. The
timestamp types themselves will not change, but the comments and variable names
should clearly indicate the timeline they operate on. The types may change in
the future, see the Future Work section for more information.

1. **Kernel**: There are many kernel APIs that take or return a monotonic
timestamp. Some of these APIs may need to be modified to support both timelines,
or switch to using boot time entirely.
2. **Timekeeper**: Computation of UTC time should switch to using boot time.
3. **Diagnostics**: The diagnostics stack uses timestamps in many places
(archivist, logs, inspect, etc). Each of these usages must be audited for
correctness.
4. **Tracing:** Tracing currently annotates samples with monotonic timestamps,
and therefore will no longer work during periods of suspension. There are strong
use cases for having traces in these intervals, so we'll need a way to
reconstruct them.
5. **Starnix**: All usages of `CLOCK_BOOTTIME` in Starnix will need to switch to
using the boot timeline.
6. **Forensics**: Crash reporting and Cobalt both contain monotonic timestamps;
these may or may not need to change to using boot timestamps.
7. **Developer Tooling**: FFX may need to add features to get the current boot
time, and may need to audit its usage of monotonic time when retrieving logs.
8. **System Libraries**: Some language specific features, like async executors
or time APIs in the standard library, may need to account for the new timeline.
9. **Software Delivery**: Omaha-client (and potentially other components in the
SWD stack) rely on monotonic time, and may need to switch timelines.
10. **Netstack**: Netstack3 uses timers that may or may not need to be switched
to using the boot timeline.

## Implementation

Implementation will proceed in several stages.

### Stage 1: Kernel Support for Boot Time

The boot timeline will be introduced in the Zircon kernel, and internal
structures will be modified to work with the new timeline. The boot timeline
will then be exposed to userspace. This will require some API changes, so
another RFC will be needed to document these changes.

### Stage 2: Starnix Integration

Once the boot timeline is supported in Zircon, Starnix can use it to support
their implementation of `CLOCK_BOOTTIME` and all related downstream
functionality. Integration with Starnix can occur before integration with other
parts of the system due to its usage of the @next vDSO, which may allow for
parallelization with the approval of the RFC mentioned in Stage 1.

### Stage 3: Maintaining Functionality on Suspend-Enabled Devices

The monotonic clock will only be paused during periods of system suspension.
Therefore, we must ensure that all software that runs on suspend-enabled
devices switches over to using the boot timeline introduced in Stage 1. This
includes changes in the system libraries, diagnostics, tracing, forensics, and
timekeeper.

This can proceed in parallel with Stage 2 whenever possible.

### Stage 4: Pause the Monotonic Clock during Suspend-To-Idle

Once all code running on suspend-enabled devices has been fixed, we can pause
the monotonic clock during suspension. This may highlight additional issues
(see the unknowns section for more information). If these issues break critical
user journeys, the CL pausing the clock will be reverted and the issue will be
fixed. If a critical user journey is not broken, then the clock will remain
paused and the issue will be fixed-forward.

### Stage 5: Maintaining Functionality on Suspend-Disabled Devices

Software that runs only on suspend-disabled devices and must be aware of time
spent suspended should then be modified to use boot time where necessary. There
is less time pressure here, as support for suspend-to-idle is lower priority on
these devices. Changes in this stage include software delivery and developer
tooling.

Netstack3 will also be updated in this stage; even though it runs on
suspend-enabled devices, its usage of timers will not break when the monotonic
clock is paused, so we need not block on it.

## Future Work

### Wake Vectors

We may wish to support resuming the system at a given point on the boot
timeline. Doing so will require us to formalize our notion of wake vectors in
the system and establish semantics for the creation of time based wake vectors.
This is left to a future RFC.

### Timeline Aware FIDL Timestamps

FIDL currently represents all timestamps using [zx.Time], which is aliased to an
`int64`. Eventually, this type would benefit from:

1. Storing, or otherwise accounting for, its reference timeline, or
1. Being replaced with distinct time types for each reference timeline.

Deciding between these approaches is left to a future RFC, as rolling out the
new timeline and understanding common usage patterns will better inform our
decision.

## Performance

The introduction of a second timeline and the pausing of the monotonic timeline
will not inherently impact performance. However, the design decisions made in
future RFCs surrounding how various APIs interact with boot time could
potentially have performance implications. Those implications should be
discussed in the associated RFCs and design documents.

## Ergonomics

This change will make reasoning about time in Fuchsia more complicated. Code
authors will need to think about whether their code needs to be made aware of
suspension-intervals before using time APIs. However, most programs should be
able to use the monotonic timeline and ignore periods of suspension since no
userspace threads will run during that time.

## Backwards Compatibility

As discussed in the Design and Implementation sections, there is some code that
currently uses the monotonic clock and may not function correctly after that
clock is paused during suspend-to-idle. This code will be migrated to the boot
timeline before we pause the clock as described in the Implementation section.

## Security considerations

There are several security critical operations that require the presence of a
trustworthy source of time. These include throttling, verifying tokens, and
verifying certificates. Today, throttling and token verification require a
Trusted Execution Environment (TEE) and/or the cloud and thus are not a major
concern of Fuchsia as a high-level OS.

Certificate verification is used across Fuchsia to verify secure transport
(TLS) for HTTPS, SSH, Omaha, TUF, and others. This requires high
trustworthiness in the time source, i.e. the time source must reflect the
physical reality of time within a margin of minutes. This need is currently
addressed by Timekeeper, which exclusively adjusts the UTC clock from which
time clients draw time from. Therefore, as mentioned earlier in the RFC,
Timekeeper must switch to using boot time. Note that Timekeeper already
prevents clients from retrieving time until trustworthiness and precision are
established, so there is no additional work needed to make this operation
secure.

Note that any usage of boot time outside of Timekeeper and its clients shall
NOT handle critical security use cases; those use cases should go through TEE
or consume the UTC clock presented by Timekeeper.

## Privacy considerations

This proposal does not impact system privacy.

## Testing

There are two separate categories of code that must be tested:

1. We must verify that code using the monotonic timeline is unaffected by
pausing during suspension. We aim to test this using CQ and extensive local
testing with autosuspend.
2. We must verify that all areas of code that need to switch to the boot
timeline function correctly. The testing plan for each of these areas is left to
those code owners and will be specified in future documentation and/or RFCs.

## Documentation

At a high level, we will add a page to fuchsia.dev describing the new boot
timeline and how it behaves. We will also modify the existing monotonic
documentation to reflect that it will pause during suspend-to-idle.

Each area of code mentioned in the design section is then responsible for
updating their documentation however they see fit.

## Drawbacks, alternatives, and unknowns

### Alternatives

One could envision alternative solutions in which we do not pause the clock
during suspend, thus removing the need for a new boot timeline. Unfortunately,
that does not meet our platform requirements:

1. **Expectations in Starnix**: Linux defines CLOCK\_MONOTONIC to
"[not count time that the system is suspended.](https://man7.org/linux/man-pages/man2/clock\_gettime.2.html)."
Therefore, Starnix must provide the same guarantee. We could implement this
using userspace clocks, but this presents significant complications as Starnix
and Fuchsia would have different clock behaviors, meaning that any IPC or
operation that involved time and timestamps would have to account for the
discrepancy.
2. **Expectations in Some Existing Code:** Some code was written assuming
monotonic time won't pause.  Some was written assuming it would. As part of the
[Suspend-To-Idle RFC], we decided that pausing the monotonic clock is lesser of
evils.
3. **Avoiding Timer "Wake-Storms"**: Consider a situation in which the system
suspends for a long period of time, and the monotonic clock continues to
progress during this period. Any number of timers set by programs in the system
may hit their deadline, but the system will not execute their callbacks until it
resumes from suspend. This could lead to scenarios in which we spend a
significant amount of time running timer callbacks on resume.

### Unknowns

The monotonic timeline is used in nearly every program that runs on Fuchsia. As
mentioned in the previous section, some of this code was written with the
assumption that the monotonic clock would continue during periods of
suspension. This will break once we pause the clock, and will likely do so in
subtle ways.

We have tried, to the best of our ability, to identify and enumerate all of the
areas of code that will need to switch to the new timeline. We've done this by
not only looking through the code, but also by talking extensively with many
stakeholders across the organization. We are confident that we have found most
of the potential breakages.

However, there are likely call-sites that we have missed, or that we have
incorrectly assumed are fine using the monotonic timeline. Unfortunately,
there's not an easy way to identify these; they are unknown unknowns that will
only present themselves after we pause the clock during suspend-to-idle and do
extensive testing of system suspension.

## Prior art and references

RFC-0230: [Suspend-To-Idle RFC]

<!-- xrefs -->
[Suspend-To-Idle RFC]: /docs/contribute/governance/rfcs/0230_suspend_to_idle.md
[zx.Time]: https://cs.opensource.google/fuchsia/fuchsia/+/main:zircon/vdso/zx_common.fidl;l=20;drc=258db6c2f3e958e5f908b62c3e67aafba4bbc207