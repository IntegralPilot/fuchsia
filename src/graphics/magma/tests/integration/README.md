Magma Conformance Testing
=========================

These tests are necessary but not sufficient to validate a Magma system driver implementation.

They cover basic usage of the Magma C API, and are generally not capable of formatting
hardware-specific commands, so cannot stress actual command execution.  For more in depth
driver validation, a test suite such as the Vulkan CTS is required.

The tests are not hermetic in that they require access to the hardware device.
