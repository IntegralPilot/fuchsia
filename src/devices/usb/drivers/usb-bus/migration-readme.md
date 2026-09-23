# USB Bus Driver Migration Status

This document tracks the migration and modernization status of the `usb-bus`
driver.

## Overview

The `usb-bus` driver is a core component of the Fuchsia **USB Host stack**. It
acts as a middle layer between **Host Controller Interface (HCI)** drivers
(e.g., XHCI) and the various **USB device drivers** (e.g., USB Mass Storage
(UMS), HID, etc.) that manage external hardware. Its primary responsibilities
include device enumeration, endpoint management, and routing USB requests from
class drivers to the HCI.

## Banjo to FIDL Migration

The `usb-bus` driver is currently in a **dual-mode** state, supporting both
Banjo and FIDL interfaces to maintain compatibility while transitioning to
modern Fuchsia standards.

| Interface / Protocol | Role | Status | Notes |
| :--- | :--- | :--- | :--- |
| `UsbHci` (Banjo) | **Client** | **Active** | Legacy protocol for data-path/control to XHCI. |
| `UsbBusInterface` (Banjo) | **Server** | **Active** | Legacy callbacks for HCI to report devices. |
| `fuchsia.hardware.usb` (Banjo) | **Server** | **Active** | Legacy protocol for class drivers (e.g., UMS). |
| `fuchsia.hardware.usb.hci/UsbHci` | **Client** | **Implemented** | FIDL Management API for HCI control plane. |
| `fuchsia_hardware_usb_hci/UsbHciInterface` | **Server** | **Implemented** | FIDL callback for device lifecycle events. |
| `fuchsia.hardware.usb/Usb` | **Server** | **Implemented** | Modern FIDL API for class drivers. |

## Migration Burn-down List

Modernizing the USB stack requires migrating the following drivers from legacy
Banjo protocols to FIDL.

### 1. Host Stack (Direct `usb-bus` Counterparts)
These drivers interact directly with `usb-bus` using `UsbHci` or `Usb` protocols.
*   **HCI**: `xhci`.
*   **Platform**: `usb-host`, `usb-virtual-bus`, `usb-hub`.
*   **Storage**: `ums` (USB Mass Storage).
*   **Input**: `usb-hid`.
*   **Media**: `usb-audio`, `usb_video`.
*   **Serial**: `ftdi`, `usb-cdc-acm`.
*   **Network**: `asix-88772b`, `asix-88179`, `rndis-host`.
*   **Other Connectivity**: `qmi-usb-transport`, `bluetooth-hci-usb`.

### 2. Peripheral Stack (Related Modernization)
These drivers use the peripheral equivalent protocols (e.g., `UsbFunction`) but
often rely on `fuchsia.hardware.usb` (Banjo) for shared types like
`UsbRequest` (the legacy `usb_request_t` for data transfers) and standard
descriptors like `UsbSetup` or `UsbDeviceDescriptor`.
*   **Platform**: `usb-peripheral`, `usb-peripheral-test`.
*   **Functions**: `usb-adb-function`, `usb-fastboot-function`, `ums-function`.
*   **Network**: `usb-cdc-function`, `rndis-function`.
*   **Others**: `usb-harriet`.

## Current Testing State

The current unit test implementation (`tests/usb-device.cc`) utilizes established
mocking patterns to verify driver behavior.

*   **DDK Mocking**: Relies on `mock-ddk` (`MockDevice`) for driver lifecycle
    simulation. This is a current dependency targeted for future migration to the
    Driver Testing Framework (DTF).
*   **Protocol Fakes**: Implements a `FakeHci` supporting both `UsbHciProtocol`
    (Banjo) and `UsbHciInterface` (FIDL) to emulate hardware controller
    callbacks.
*   **Dispatcher Environment**: Uses `fdf_testing::DriverRuntime` to support
    units tests with asynchronous dispatchers.
*   **Time Handling**: Uses a `UsbWaiterInterface` (`FakeTimer`) to provide
    deterministically controlled wait and timeout behavior in tests.

## Integration Testing

Besides unit tests, the driver is a primary test case for the
`usb-virtual-bus-test` suite (at `//src/devices/usb/drivers/usb-virtual-bus/tests`),
which validates the entire stack's behavior under realistic conditions
(including rapid connect/disconnect and re-enumeration scenarios).

## Future Work

- [ ] Complete the migration of the core data-path (`RequestQueue`) to FIDL
  once performance parity is verified and downstream class drivers (e.g., UMS)
  are updated.
- [ ] Migrate from `ddktl` and `mock-ddk` to the new Driver Framework (DFv2)
  and its corresponding Driver Testing Framework.
