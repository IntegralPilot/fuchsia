# Drivers

Important: This section contains information that is specific to the new
version of the driver framework (DFv2).

Drivers provide software interfaces for communicating with hardware (or virtual)
devices that are embedded in or connected to a system. In Fuchsia, drivers are
user-space [components][components]. Like any other Fuchsia component, a driver
is software that exposes and receives FIDL capabilities to and from other
components in the system. Using these FIDL calls, Fuchsia components interact
with drivers, which are bound to specific devices in the system.

Similar to Fuchsia’s component framework, which manages Fuchsia components, the
[driver framework][driver-framework] manages the lifecycle and topology of
all devices (known as [nodes][nodes]) and drivers in a Fuchsia system.

## Table of contents

*  [**Driver framework (DFv2)**][driver-framework]: Explains the core concepts
   and structure of Fuchsia's new driver framework.
*  [**Drivers and nodes**][nodes]: Explains the fundamental concepts of nodes,
   which are the building blocks in DFv2.
*  [**Driver binding**][driver-binding]: Explains the process of how the driver
   manager matches and binds a driver to a node.
*  [**Driver communication**][driver-communication]: Explains different
   communication methods: driver-to-driver and non-driver-to-driver.
*  [**Mapping a device's memory in a driver**][mapping-memory]: Explains
   how a device's memory region is mapped in a driver host.
*  [**Driver dispatcher and threads**][driver-dispatcher]: Explains how
   dispatchers allow drivers to schedule asynchronous work on threads.
*  [**Driver dispatcher performance**][driver-dispatcher-performance]:
   Discusses performance aspects of a dispatcher's inlined and non-inlined
   calls.

To learn more about creating DFv2 drivers, see the
[**DFv2 driver development**][dfv2-overview] section.

<!-- Reference links -->

[components]: /docs/concepts/components/v2/README.md
[driver-framework]: driver_framework.md
[nodes]: drivers_and_nodes.md
[driver-binding]: driver_binding.md
[driver-communication]: driver_communication.md
[mapping-memory]: mapping-a-devices-memory-in-a-driver.md
[driver-dispatcher]: driver-dispatcher-and-threads.md
[driver-dispatcher-performance]: driver-dispatcher-performance.md
[dfv2-overview]: /docs/development/drivers/dfv2-overview.md
