# USB Hardware Policy FIDL interfaces

The fuchsia.hardware.usb.policy interfaces are the portions of the USB
policy solution that are served by driver components.

The first of these, Controller, is intended to be served by the driver
which directly interacts with the USB controller hardware. An example
of this category of drivers is the dwc3 driver.

Planned expansions of these interfaces include covering more types of
USB driver components, like phy drivers, as well as additional categories
of information flow, such as telemetry, watchdog behavior, and driver
restarts during testing and observed failures.
