This library provides a fake replacement for a platform device. It can act as a
FIDL server for the fuchsia.hardware.platform.device/Device FIDL protocol. This
protocol is how drivers normally communicate with a platform device that they
are bound to. Tests for drivers that require communication with a platform
device should offer the fake platform device's FIDL service to the driver's
incoming namespace.
