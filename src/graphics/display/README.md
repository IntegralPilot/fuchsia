# The Fuchsia display stack

The code here is responsible for driving the display engine hardware in the
systems supported by Fuchsia.

The [Display hardware overview][display-hardware-overview] document introduces
many terms needed to understand display code, and is a good background reading.

The [Display configuration states][display-config-states] document covers some
important terms used throughout the stack.

The [Principles for modern register definitions][display-register-definitions]
document summarizes the conventions adopted by display drivers.

## Stack overview

### The FIDL interface for display clients

The official entry point to the display driver stack is the
[`fuchsia.hardware.display/Coordinator`][display-coordinator-fidl] FIDL
interface. This FIDL interface will eventually become a part of the
[Fuchsia System Interface][fuchsia-system-interface], and will be covered by ABI
stability guarantees.

The interface's consumers are called **display clients**. This is a reference to
the fact that consumers use FIDL clients for the Coordinator interface.

This interface is currently only exposed inside the Fuchsia platform source
tree. The existing consumers of the interface are [Scenic][scenic],
[virtcon][virtcon], and the [recovery UI][recovery-ui]. The last two consumers
use the [Carnelian][carnelian] library. There are no plans of supporting any new
consumers.

### The Display Coordinator

The Display Coordinator maintains the state needed to multiplex the display
engine hardware across multiple display clients. The Coordinator's design aims
to allow for multiple display clients and multiple display engine drivers. This
goal was sacrificed in a few places, in the name of expediency.

Whenever possible, data processing is implemented in the Coordinator, reducing
the implementation complexity of hardware-specific display drivers. For example,
the Coordinator performs all the hardware-independent validation of the layer
configurations received from display clients, before passing the configurations
to the display engine drivers.

The Coordinator implements the `fuchsia.hardware.display/Coordinator` FIDL
interface, described above, to serve requests from display clients such as
Scenic.

The Display Coordinator is currently implemented as the `coordinator` driver in
the Fuchsia platform source tree. The `fuchsia.hardware.display/Coordinator`
FIDL interface is currently served under the `display-coordinator` class in
[`devfs`][devfs].

### The interface between the Display Coordinator and engine drivers

The Coordinator uses the
[`fuchsia.hardware.display.engine/Engine`][display-engine-fidl] FIDL interface
to communicate with hardware-specific display drivers. This FIDL interface will
soon become a part of the Fuchsia System Interface, and will be covered by API
stability guarantees.

### Display engine drivers

Display engine drivers contain hardware-specific logic for driving a display
engine.

Display engine drivers are currently built on top of [DFv2 (Driver Framework
v2)][dfv2]. Drivers communicate with the Driver Framework via the [DFv2 (Driver
Framework v2) FIDL interface][dfv2-fidl].

Many display engine drivers interface with the Display Coordinator using the
[api-protocols][display-api-protocols] and [api-types][display-api-types]
libraries. These libraries are ergonomic wrappers for proxies generated from the
FIDL interface mentioned above.

Most display engine drivers were initially built on top of [DFv1][dfv1] and
[migrated to DFv2][dfv2-migration]. The DFv1 legacy may show up in the drivers'
structure, variable names, or comments. We consider this to be technical debt
that causes cognitive overhead, and we aim to clean it up.

## Coding standards

We aim to have the display driver stack closely follow all applicable style
guides. We see unnecessary variations as a source of complexity.

The following list aims to include all the style guides referenced in our code
reviews.

* [Fuchsia C++ style guide][style-fuchsia-cpp]
* [Google C++ style guide][style-google-cpp]
* [Abseil C++ tips of the week][style-abseil-totw]
* [Abseil performance guide][style-abseil-perf]
* [TotT (Tech on the Toilet) in the Google Testing Blog][style-google-testing]
* [Fuchsia documentation standards][style-fuchsia-doc-standards]
* [Fuchsia documentation style guide][style-fuchsia-docs]
* [Google developer documentation style guide][style-google-docs]
* [Fuchsia FIDL style guide][style-fuchsia-fidl]
* [Fuchsia FIDL API rubric][style-fuchsia-fidl-rubric]
* [Software Engineering at Google][style-google-swe-book]

[carnelian]: /src/bringup/bin/virtcon/README.md
[devfs]: /docs/concepts/drivers/driver_communication.md#service_discovery_using_devfs
[display-api-protocols]: /src/graphics/display/lib/api-protocols/README.md
[display-api-types]: /src/graphics/display/lib/api-types/README.md
[dfv1]: /docs/development/drivers/concepts/fdf.md
[dfv1-protocol]: /docs/development/drivers/concepts/device_driver_model/protocol.md
[dfv2]: /docs/concepts/drivers/driver_framework.md
[dfv2-fidl]: /docs/concepts/drivers/driver_framework.md#fidl_interface
[dfv2-migration]: /docs/development/drivers/migration/migrate-from-dfv1-to-dfv2.md
[display-config-states]: /docs/development/drivers/driver_guides/display/config-states.md
[display-hardware-overview]: /docs/development/drivers/driver_guides/display/hardware.md
[display-register-definitions]: /docs/development/drivers/driver_guides/display/register-definitions.md
[display-coordinator-fidl]: /sdk/fidl/fuchsia.hardware.display/coordinator.fidl
[display-engine-fidl]: /sdk/fidl/fuchsia.hardware.display.engine/engine.fidl
[fuchsia-system-interface]: /docs/concepts/packages/system.md
[recovery-ui]: /src/recovery/lib/recovery-ui/BUILD.gn
[scenic]: /docs/concepts/ui/scenic/index.md
[style-abseil-perf]: https://abseil.io/fast/
[style-abseil-totw]: https://abseil.io/tips/
[style-fuchsia-cpp]: /docs/development/languages/c-cpp/cpp-style.md
[style-fuchsia-docs]: /docs/contribute/docs/documentation-style-guide.md
[style-fuchsia-doc-standards]: /doc/contribute/docs/documentation-standards.md
[style-fuchsia-fidl]: /docs/development/languages/fidl/guides/style.md
[style-fuchsia-fidl-rubric]: https://fuchsia.dev/fuchsia-src/development/api/fidl/
[style-google-cpp]: https://google.github.io/styleguide/cppguide.html
[style-google-docs]: https://developers.google.com/style
[style-google-swe-book]: https://abseil.io/resources/swe-book
[style-google-testing]: https://testing.googleblog.com/search/label/TotT
[virtcon]: /src/bringup/bin/virtcon/README.md
