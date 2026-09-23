<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0255" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Summary

This RFC proposes adding a core component named System Activity Governor (SAG)
to manage the power level of the system. This document presents the requirements
of SAG, its policies, and the flows used in its implementation to enable
management of the system power level. This RFC can be viewed as a part of the
implementation for [RFC-0230: Suspend-To-Idle in Fuchsia].

### Goals

- Enable management of when suspend-to-idle occurs.
- Provide statistics about suspension to further future power improvements and
  enable monitoring fleet-wide power consumption.

### Non-goals

- Implement transitions of specific hardware systems to different power states
  including, but not limited to, CPU, GPU, memory, network interfaces, etc.
- Implement logic to trigger task suspension, e.g. `zx_system_suspend_enter`.


## Motivation

Fuchsia is a general purpose operating system designed to power a diverse
ecosystem of hardware and software. To satisfy the needs for power-sensitive
hardware and software, Fuchsia must be able to manage the power consumption of
the hardware platform it is running on. This includes having the ability to
transition the CPU to low power mode, trigger system-wide task suspension,
activate memory self-refresh, and any number of other power-saving hardware
features. A set of hardware configurations or states designed to lower power
consumption are grouped into a concept called a **suspend state**.

Many systems support multiple suspend states. Each suspend state has a set of
hardware states, power usage, and resume latency (the time delay to exit the
suspend state). Each system may define the same suspend state in concept
(Suspend-to-Idle) which maps to a completely different set of hardware
configurations.

For example, Fuchsia may be running on two systems with different CPUs. One CPU
may support a power state with a lower power consumption than the other;
however, both systems may consider a transition to the lowest power state to be
part of its operations to enter a Suspend-to-Idle state.

When considering this topic, the state of the platform is as follows:

- [RFC-0230: Suspend-To-Idle in Fuchsia] defines the high-level requirements and
  design for Suspend-to-Idle. Currently, this is the only suspend state
  supported by the Fuchsia platform.
- [RFC-0250: Power Topology] defines the power topology which underpins future
  power management systems within Fuchsia.
- A separate RFC defines the component that manages the power topology
  at runtime: Power Broker.
- A variety of signals from various subsystems must be interpreted to coordinate
  a transition into Suspend-to-Idle.

In order to implement [RFC-0230: Suspend-To-Idle in Fuchsia] using the
[RFC-0250: Power Topology], a component implementing the policies to manage
a transition to Suspend-To-Idle needs to be created: System Activity Governor.

## Stakeholders

_Facilitator:_

Adam Barth <abarth@google.com>

_Reviewers:_

- Aidan Wolter <awolter@google.com> (Software Assembly)
- Andres Oportus <andresoportus@google.com> (Platform Drivers)
- Gary Bressler <geb@google.com> (Component Framework)
- Gurjant Kalsi <gkalsi@google.com> (Platform Drivers)
- Harsha Priya N V <harshanv@google.com> (Driver Framework)
- Justin Mattson <jmatt@google.com> (Driver Framework)
- Kyle Gong <kgong@google.com> (Power)
- Mukesh Agrawal <quiche@google.com> (UI)
- Nick Maniscalco <maniscalco@google.com> (Zircon Kernel)
- Onath Dillinger <claridge@google.com> (Power)

_Consulted:_

- Alice Neels <neelsa@google.com> (UI)
- David Gilhooley <dgilhooley@google.com> (Component Framework)
- Didi She <didis@google.com> (Power)
- Eric Holland <hollande@google.com> (Power)
- Filip Filmar <fmil@google.com> (UI)
- Guocheng Wei <guochengwei@google.com> (Starnix)
- HanBin Yoon <hanbinyoon@google.com> (Platform Drivers)
- John Wittrock <wittrock@google.com> (Software Delivery)
- Novin Changizi <novinc@google.com> (Driver Framework)
- Suraj Malhotra <surajmalhotra@google.com> (Driver Framework)

_Socialization:_

This design was discussed with the Power team and subsystem owners who manage
the kernel, drivers, driver framework, component framework, product policy, and
power framework.

## Requirements

A system that supports a transition to suspend must support the use cases of a
Fuchsia product along with the hardware requirements of the suspend state. To
properly facilitate transitions to suspend while upholding the use cases of the
product, SAG must meet the following requirements:

1. **Fuchsia components (including drivers) must be able to prevent transitions
  to suspend when critical operations are in progress.** For example, consider
  the scenario where the user is watching a video. The device should not attempt
  to suspend the system while the video is playing; otherwise, the user will
  have a poor experience.
1. **To enable product-driven suspend policies, statistics about suspensions
  must be collected and exposed .** For example, consider a product where
  transitions to suspend may only be triggered at least 1 second after the
  previous resume. In order for product components to support this feature, it
  must be able to calculate the time since the previous resume transition.
1. SAG will just be responsible for determining *when* to transition. The
  definition and facilitation of a transition to each suspend state will be
  handled by a component that implements the
  `fuchsia.hardware.power.suspend.Suspender` protocol which SAG uses. This RFC
  defers defining the full protocol, but `Suspender` **MUST** provide the
  following functionality:

    - List suspend states supported by the hardware.
    - Transition the hardware to a suspend state.

For this design, SAG acts as a _governor (or limiter) on the system's ability to
execute code_ which is fulfilled by a *Fuchsia Suspend HAL* (which implements
the `Suspender` protocol described above) to transition the system to suspend.
To accommodate this concept, we make the following assumptions:

- The **hardware platform** refers to the collection of hardware and software
  systems which enable a computing device to execute code in this RFC. This
  typically includes the CPU, memory, operating system, and other controls
  required for these to function (clock trees, power domains, etc.). This
  explicitly **DOES NOT** include traditional peripheral components such as
  microcontroller units, wireless radios, display controllers, secondary storage
  (solid-state drive, eMMC), etc.
- The power broker component **MUST** be present in all products that wish to
  support hardware platform suspension.
- Fuchsia components will not be executing on the CPU during suspension
  including SAG itself. Firmware that runs on microcontrollers or other
  peripheral hardware *may* continue to process data. When the hardware platform
  enters a suspend state, CPUs *may* be taken offline, the scheduler *may*
  update its policies, and peripherals *may* change configuration.
- Fuchsia components **MUST** integrate (directly or indirectly) with the power
  topology to allow and/or prevent transitions to suspend. This centralized
  accounting mechanism enables SAG to know what the rest of the system requires
  without having to know the state of those components.
- Fuchsia components that do not integrate with the power topology MUST NOT
  expect uninterrupted execution. This requires any component that does expect
  uninterrupted execution in order to perform critical tasks (e.g. receive data
  from a network or peripheral) to account for hardware platform suspension by
  directly or indirectly interacting with power framework in one of the methods
  described in [Preventing Suspend](#preventing_suspend).
- The hardware platform **MUST NOT** suspend while a waking interrupt is
  pending. This is important for all drivers since it gives them a chance to
  service their interrupt regardless of their interaction with the power
  framework. Drivers that need to do more processing after ACKing an interrupt
  **MUST** interact with the power framework to prevent hardware platform
  suspension.
- SAG **MUST** be available to drivers and eager components that are power-aware
  and present in the first file systems that are read by the kernel (e.g. bootfs
  from [RFC-0167: Packages in early userspace
  bootstrapping](0167_early_boot_packages.md)). Without this, components that
  are power-aware will not be able to set up power configurations that prevent
  system suspension until SAG becomes available later. Additionally, during
  suspend transitions, higher level file systems may be configured such that
  they power down earlier than drivers that exist in lower level file systems.
  This would create power element dependency issues.
- SAG **WILL** drive the hardware platform towards its lowest power level
  whenever possible using power topology concepts. Ultimately, how long the
  device enters/stays in this lowest power level or not is determined by product
  behavior and policies, environmental conditions, and user behavior.

## Design

System Activity Governor consists of the following parts:

1. A power element
1. Business logic for initialization, state management, and handling for power
   level changes.
1. A FIDL service to provide access to statistics about suspend/resume.
1. FIDL services to allow clients to keep the hardware platform awake and be
   notified of suspend/resume transitions.

The exact definition of the FIDL services will be deferred to an API
review/calibration session.

### Power Topology

> See [RFC-0250: Power Topology] for info on power topology diagram conventions.

![Power Topology
diagram](resources/0255_system_activity_governor/InternalPowerTopology.svg)

SAG creates and manages a power element, [`Execution State`], that represents
the ability for the hardware platform to execute code. This power element
aggregates signals from other components to determine when a suspend transition
should be attempted.

By creating a power element, SAG allows other components to:

- Raise the power level of the execution state of the hardware platform for as
long as they hold an assertive claim
- Give reasoning for *why* the hardware platform should continue executing code
via power elements and their dependency chains
- Respond to changes in the execution state by holding opportunistic claims.
This also allows the SAG implementation to produce side effects in response to
power level changes for each dependent of `Execution State`.
- Most importantly, the [RFC-0250: Power Topology] requires that a power element
tends towards its lowest power level. This naturally leads to a *governing*
action on the execution state of the hardware platform when no dependent power
elements are active.

#### Execution State

This power element maps as closely as possible to the hardware platform's
ability to execute code. It supports 3 power levels:

- Active
- Suspending
- Inactive

When the hardware platform is running normally, this element is expected to be
at the `Active` power level. While at the `Inactive` power level, the hardware
platform is expected to be either in a suspend state or actively attempting to
transition to a suspend state. Only parts of the device that are not power-aware
or are power-agnostic should be running at this time. See [Constraining a Device
by Execution State](#constraining_a_device_by_execution_state) for more info on
this design pattern. See [Suspend/Resume](#suspendresume) for more info on when
and how suspend is triggered.

The `Suspending` power level sits between the `Inactive` and `Active` power
levels. This power level exists to separate power dependencies that are *only*
allowed to run when the hardware platform is running and those that *may* need
to run when the hardware platform is transitioning between a suspend state and a
running state (in either direction). See the [Storage example](#storage_example)
below for a scenario where this distinction is useful.

### Design Patterns

#### Preventing Suspend

To prevent the hardware platform from suspending, a component can construct a
power element that has an [assertive
dependency](0250_power_topology.md#dependency_types) on `Execution State`.

Consider a product that has a media player feature. The product owner wants
media playback to prevent the hardware platform from suspending in order to
support continuous music playback. To support this feature, the component that
manages the playback state could create an assertive dependency on `Execution
State` to prevent SAG from triggering a suspend transition.

Note: An [opportunistic dependency](0250_power_topology.md#dependency_types)
would also work if and only if another power element has *already* raised
`Execution State` to the `Active` power level.

![Preventing Suspend via Execution State
diagram](resources/0255_system_activity_governor/PreventingSuspendExecutionState.svg)

In the diagram above, the Media Player component has a power element called
Playback. The Playback power element has an assertive dependency on the `Active`
power level of `Execution State`. When media playback needs to start, the Media
Player component will request a lease for the `Active` power level of `Playback`
in order to prevent the hardware platform from suspending. After the lease is
satisfied, the Media Player component can run the media playback logic without
needing to worry about the hardware platform suspending unexpectedly.

#### Constraining a Device by Execution State

One powerful pattern that a product can employ is constraining a device's power
level by the hardware platform's execution state. This can be used to tie
peripheral device power consumption to the hardware platform's suspend state. As
the hardware platform transitions to suspend, a driver can power down its device
or change its configuration. This setup may be useful for products that want to
power down specific features or subsystems when the hardware platform suspends
to reduce power consumption.

###### Audio Example

Consider a product with power-intensive audio hardware. The product owner only
wants to activate the audio hardware when the hardware platform is awake. The
product owner could represent this configuration with a power topology with an
[opportunistic dependency](0250_power_topology.md#dependency_types) on
`Execution State`.

![Constraining a Device by Execution State
diagram](resources/0255_system_activity_governor/ConstrainingADeviceByExecutionState.svg)

In the diagram above, the audio driver, called Audio in the diagram, has a power
element called Power. The audio driver has an opportunistic dependency on the
`Active` power level of `Execution State`. The audio driver can watch for its
dependencies to be fulfilled by requesting a lease from Power Broker. When
`Execution State` transitions to `Active`, The audio driver's lease on Power
will be satisfied. At that time, The audio driver can run whatever logic it
needs to activate the audio hardware. Conversely, when `Execution State` wants
to drop its power level from `Active`, the audio driver's lease on power becomes
unsatisfied. At that time, the audio driver can run logic to deactivate the
audio hardware before notifying Power Broker that its power element has dropped
its power level.

#### Handling Waking Interrupts

When the hardware platform is suspended, it may be resumed by an external
stimulus. This external stimulus typically comes in the form of a CPU interrupt
that is raised by one of the peripheral components. To properly handle a waking
interrupt, the following is expected to occur:

- Wake vectors for drivers are programmed at some point before suspension.
- When handling an interrupt:
    1. If a driver is waiting on a port for an interrupt, its interrupt handler
      thread (IHT) is scheduled by the kernel.
    1. The IHT or another component notified by it *may* create a lease to block
       hardware platform suspension.
    1. The driver communicates with the hardware and/or other components as
      required to handle the interrupt. During this time, other components may
      request leases.
    1. The driver calls `zx_interrupt_ack` once interrupt handling is done.

Note: Some drivers may need to call `zx_interrupt_ack` as soon as possible for
asynchronous interrupt/data handling in hardware. In this case, the driver (or
its delegates) should acquire a lease that raises the power level of `Execution
State` to at least the `Suspending` power level and keep the lease alive
until interrupt handling completes.

While an interrupt is being handled, SAG can run at any time since the kernel
resumes the tasks that were suspended when the hardware platform transitioned to
suspend.

##### Storage Example

Consider the [audio example](#audio_example) from [Constraining a Device by
Execution State ](#constraining_a_device_by_execution_state). While storage
hardware could follow a similar pattern to audio where it is normally turned off
when the hardware platform is suspended, it would also be off while the hardware
platform is transitioning to suspend or resuming to service a waking interrupt.
This may cause issues if a component that is currently paged out needs to power
down and perform cleanup operations before suspension. Since the order of
power-down operations is not deterministic, this simplified design is
insufficient.

One way to address this shortcoming is to allow the storage driver to run while
transitions are occurring.

![Powering Up Storage
diagram](resources/0255_system_activity_governor/PoweringUpStorage.svg)

In the diagram above, the storage driver, called Storage, has two power
elements:

- Power has an opportunistic dependency on (Execution State, Suspending).
  This dependency will be satisfied when another power element raises `Execution
  State`'s power level. As long as storage driver holds a lease on Power after
  Execution State powers up to `Suspending`, Execution State cannot drop its
  power level. This is required to allow the storage driver to watch for when it
  should power down.
- Waking Request has an assertive dependency on (Execution State,
  Suspending). This dependency forces `Execution State` to raise its power
  level. The storage driver will lease this power element when it receives a
  storage request. This is required to allow the storage hardware to power up
  *even while other parts of the system are powering down.*

### Handling System-wide Transitions

#### Boot

When SAG is first started, it will not trigger suspension until the system has
completed the boot process. While in this booting state, the `Execution State`
power element is expected to be at the `Active` power level to reflect the
reality that the hardware platform is actively running and not attempting to
suspend.

During this time, other components can create dependencies and leases that
affect `Execution State`; however, `Execution State` will always be at the
`Active` power level until boot completes. In order to exit the booting state, a
FIDL service hosted by SAG must be notified that boot has completed. This
service is expected to be used when higher layers of the system can express
their power needs, e.g. by the session when user interaction is possible.

#### Suspend/Resume

Suspend transitions will be requested while the `Execution State` power level is
`Inactive`, the lowest power level. Before starting a suspend transition, SAG
will inform registered listeners that a suspend is about to occur. To properly
handle potential races between itself, Power Broker, and the interrupt-handling
driver, SAG requires the following:

- The Fuchsia Suspend HAL **MUST** return an error if a waking interrupt has not
  been ACKed. This prevents a race where SAG runs before the interrupt handler
  has a chance to request a lease and ACK the interrupt.
- SAG **MUST NOT** change the power level of `Execution State` while a suspend
  request is in progress. When SAG resumes from suspend, the power level of
  `Execution State` should always be `Inactive`. Devices that depend on
  `Execution State` will transition to and stay at their appropriate power level
  immediately before and during the transition to suspend. SAG "locks" the power
  level of `Execution State` to ensure this by delaying the processing of power
  level changes requested by the Power Broker.
- The driver or interrupt handler **MUST** take a lease before ACKing its
  interrupt if the driver needs to execute work afterwards.

When the hardware platform resumes execution, SAG will receive a response from
the `Suspender` device indicating the result of the suspend request. SAG will
then notify registered listeners of the result of the suspend request. At that
time, listeners can request a lease that blocks hardware platform suspension.
See [Preventing Suspend](#preventing_suspend) for more info.

It is also possible for a suspend request to fail before a transition occurs.
This may be caused by a pending interrupt that will immediately wake the
hardware platform or other error in the Fuchsia Suspend HAL or kernel. When a
suspend request fails, SAG notifies listeners of the failure and waits for them
all to acknowledge the notification. This setup allows product-level components
to request leases and change the system configuration to prevent repeatedly
sending suspend requests that will ultimately fail.

In the case where no listener raises the power level of `Execution State` during
a resume transition, SAG logs this failure. This scenario can occur in the case
of crashes of listeners or on products that have no active (or an incomplete/in
development) product experience.

![Suspend/Resume
diagram](resources/0255_system_activity_governor/SuspendResume.svg)

#### Shutdown

When a shutdown or reboot is requested by a component running in the system,
component manager begins terminating components based on the capability graph.
This creates a problem where components that are keeping the hardware platform
from suspending will be terminated before SAG. This would cause SAG to
erroneously trigger a suspension while a shutdown is in progress.

To address this problem, the component responsible for coordinating shutdown
(`shutdown-shim` and/or `power-manager`) must hold a lease that keeps `Execution
State` at the `Suspending` power level. Since `shutdown-shim` and
`power-manager` are some of the last components that are left running before a
reboot, this prevents erroneous suspensions.

## Implementation

Adding this feature requires many individual changes and support from critical
subsystems before full implementation is completed. This can be broken down into
the following steps:

1. Basic suspend support
    - Trigger suspend when `Execution State` becomes `Inactive`
    - Send listener notifications on resumes.
    - Connect listener for resume notifications that will keep the system awake.
1. Connect all critical subsystems to SAG interfaces as they become power-aware.
1. Optimize suspend timing.
    - Expose presence of interrupts and/or other waking conditions from kernel
      and/or Fuchsia Suspend HAL.
    - Use kernel/Fuchsia Suspend HAL signals to delay triggering suspend
      requests that are expected to fail.
1. Continuously integrate other suspend-aware subsystems.

## Performance

The overall impact on system performance largely depends on the performance of
Power Broker. Most clients of SAG will perform a one-time call during their
initialization to set up dependencies with future interactions being indirect,
facilitated by Power Broker. Clients that are listening to SAG events have a
direct impact on system performance since they **MUST** acknowledge handling of
the resume and suspend fail notifications. We should closely monitor latency
metrics for these event handlers to ensure resume and suspend failures are
handled in a timely manner across products. Finally, latency for triggering a
suspend is reported by the Fuchsia Suspend HAL. We should monitor this latency
to ensure suspend operations are completed in a timely manner across products.
This includes monitoring latency and performance impact of reversing a suspend
request when a suspend failure occurs.

## Security considerations

The power framework established by [RFC-0250: Power Topology] defines the
security model for power topologies. SAG exposes access to power elements via a
protocol capability. Having access to this protocol allows other power elements
to make dependencies on SAG's power elements. Ultimately, SAG security relies on
guarantees provided by the Component Framework for routing the protocol
capability.

SAG has protocol capability dependencies on a Fuchsia Suspend HAL and Power
Broker. Any deviation from expected behavior in those components may cause
misbehavior in the SAG implementation. SAG will need to account for missing and
misbehaving dependencies to mitigate the potential for cascading negative
effects.

There is potential for abuse from clients that register listeners. A malicious
component may prevent proper suspend or resume by not acknowledging events in a
timely manner. It may be possible to avoid this class of issues by having a
timeout on the response.

## Privacy considerations

We expect no personally identifiable information to be collected by SAG;
however, data derived from suspend/resume behavior will be collected and exposed
by SAG via FIDL protocols and Inspect. This data will be aggregated in
off-device systems for analysis by product owners and Fuchsia engineers. This
will require a privacy review for the implementation.

## Testing

SAG will be tested by a collection of integration tests and end-to-end tests.
Integration tests will ensure SAG transitions between internal states and sends
suspend requests properly.

Clients can test their implementations with a set of test support libraries that
use Power Broker and SAG but remove the Fuchsia Suspend HAL. This test support
library will allow a client to invoke specific states in SAG to ensure client
components respond correctly to those changes. This includes ensuring all side
effects triggered by power level transitions are handled appropriately. A
complete test suite for a component that depends on SAG's power elements should
include the following scenarios:

- No power framework
- SAG in the initial, booting state with locked power levels
- `Execution State` at `Active` power level
- `Execution State` at `Suspending` power level
- `Execution State` at `Inactive` power level
- (If applicable) No suspend during critical operations

## Documentation

Extensive documentation will be needed to explain SAG's role in the system,
configuration (including expectations from product owners), API patterns, and
test support libraries. The documentation should include examples and best
practices for manipulating SAG power elements and using its API appropriately.
Examples should include step-by-step guides for scenarios listed in the [Design
Patterns](#design_patterns) section above.

This documentation should be part of a larger set of documentation for the power
framework and its concepts.

## Drawbacks, alternatives, and unknowns

Drawbacks to implementing this proposal build on drawbacks for the power
framework in general.

Signals to trigger a suspend are decentralized. No single component can
guarantee suspend will occur with this API unless the product owner organizes
components to enforce this pattern. This leaves the behavior of the system
ambiguous when viewed from an API use perspective. For example, when a component
that has a lease on `Execution State` drops the lease, SAG may not suspend if
other components have a lease. A component will only know a suspend is triggered
by registering a listener with SAG which increases complexity.

[RFC-0230: Suspend-To-Idle in Fuchsia] notes the following:
> If the user space provides a resume latency requirement, Fuchsia should
> honor the same. If no resume latency is provided, Fuchsia should enter
> Suspend-To-Idle with the lowest possible resume latency.

This RFC does not consider this requirement since only one suspend state is
currently supported by the platform.

We have not identified all possible power control patterns that exist for
drivers and hardware in the field, so this design may need to evolve as these
patterns are discovered. This includes but is not limited to resume latency
considerations, support for more suspend states, etc. These design changes will
be codified in follow-up RFCs as needed.

## Prior art and references

- [Android SystemSuspend
  service](https://source.android.com/docs/core/power/systemsuspend)
- [Linux System Sleep
  States](https://www.kernel.org/doc/html/next/admin-guide/pm/sleep-states.html#system-sleep-states)
- [Windows Power Policy
  Ownership](https://learn.microsoft.com/en-us/windows-hardware/drivers/wdf/power-policy-ownership)

[RFC-0230: Suspend-To-Idle in Fuchsia]: 0230_suspend_to_idle.md
[RFC-0250: Power Topology]: 0250_power_topology.md
[`Execution State`]: #execution_state
