# PCI

The PCI bus driver provides management of the PCI bus in applicable systems and allows device drivers to be written for and bind to PCI devices in the system.

# Architecture

## Source

### Location

* [`//src/devices/pci/drivers/pci`](https://cs.opensource.google/fuchsia/fuchsia/+/main:src/devices/pci/drivers/pci/)

### Organization

Bus level operations are handled in `bus.cc`. Device specific operations are in `device_*.cc`, and FIDL / Banjo protocol implementations are in `fidl.cc` and `banjo.cc` respectively. The interface between a Device and the Bus is the `BusDeviceInterface` specified in `bus_device_interface.h`. The service implementation `fuchsia.hardware.pci/Service` used by `lspci` is in `device_service.cc`.

### Build Targets

* [`//src/devices/pci/drivers/pci:bus-pci`](https://cs.opensource.google/fuchsia/fuchsia/+/main:src/devices/pci/drivers/pci/BUILD.gn;l=86) for the driver.

## FIDL

### Dependent Protocols

The PCI bus driver binds to [`fuchsia.hardware.pciroot/Pciroot`](https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/banjo/fuchsia.hardware.pciroot/pciroot.fidl) and receives platform level information for ACPI (if applicable), interrupts, interrupt routing, and bus configuration.

### Implemented Protocols

* [`//fuchsia/sdk/fidl/fuchsia.hardware.pci/pci.fidl`](https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.hardware.pci/pci.fidl)
  * `fuchsia.hardware.pci/Device`: Provides the interface to develop PCI device drivers
  * `fuchsia.hardware.pci/Service`: Enables device information export for use with lspci

## Libraries

* [`//src/devices/pci/lib/device-protocol-pci:device-protocol-pci`](https://cs.opensource.google/fuchsia/fuchsia/+/main:src/devices/pci/lib/device-protocol-pci/BUILD.gn;l=7)
  * Provides helper methods `MapMmio` and `ConfigureInterruptMode` for use with drivers, as well as convenience methods for constructing a client end for `fuchsia.hardware.pci/Device` calls.
* [`//src/devices/pci/lib/pci:pci`](https://cs.opensource.google/fuchsia/fuchsia/+/main:src/devices/pci/lib/pci/BUILD.gn;l=9)
  * Provides base implementations for the `PciRootHost` interface which provides a way for a platform to implement a backing driver that can service multiple `fuchsia.hardware.pciroot/Pciroot` instances to support multiple PCI buses in the system.
* [`//src/devices/pci/testing:pci-protocol-fake`](https://cs.opensource.google/fuchsia/fuchsia/+/main:src/devices/pci/testing/BUILD.gn;l=14)
  * Provides a fake `fuchsia.hardware.pci/Device` implementation for use in unit testing, allowing users to create fake PCI devices that match the configuration of their real hardware.

# Testing

## Tests

* [`//src/devices/pci/drivers/pci:tests`](https://cs.opensource.google/fuchsia/fuchsia/+/main:src/devices/pci/drivers/pci/BUILD.gn;l=117)
  * `pci-driver-test`
  * `pci-unit-test`
  * `pci-unit-test-fake_ddk`
* [`//src/devices/pci:tests`](https://cs.opensource.google/fuchsia/fuchsia/+/main:src/devices/pci/BUILD.gn;l=5)
  * `device-protocol-pci-test`
  * `pci-protocol-fake-test`
  * `pci-roothost-test`

## Fakes

Fakes exist for most major classes in the PCI bus driver and can be found in [`//src/devices/pci/drivers/pci/test/fakes/`](https://cs.opensource.google/fuchsia/fuchsia/+/main:src/devices/pci/drivers/pci/test/fakes/)

# Debugging

## Inspect

Inspect data for both the Bus and all devices are exported via `bootstrap/driver_manager` under `class/pci/*.inspect`:

```
$ ffx inspect show bootstrap/driver_manager --file class/pci/000.inspect
bootstrap/driver_manager:
  metadata:
    filename = class/pci/000.inspect
    component_url = fuchsia-boot:///driver_manager#meta/driver_manager.cm
    timestamp = 1382682707618
  payload:
    root:
      Bus:
        acpi devices = ["00:1f.0", "00:1f.3", "00:00.0", "00:01.0", "00:02.0", "00:03.0", "00:04.0", "00:05.0", "00:06.0"]
        bus end = 0xff
        bus start = 0x0
        ecam = 0x1047cbaa000
        name = PCI0
        segment group = 0x0
        vectors = ["0x13", "0x12", "0x11", "0x17", "0x16", "0x10", "0x15", "0x14"]
        irq routing entries:
          0x0:
            device id = 0x0
            pins = ["0x14", "0x15", "0x16", "0x17"]
            port device id = 0xff
            port function id = 0xff
[...]
          0xf:
            device id = 0xf
            pins = ["0x17", "0x14", "0x15", "0x16"]
            port device id = 0xff
            port function id = 0xff
      Devices:
        00:00.0:
        00:01.0:
          BARs:
            0:
              0. Initial = 0xc000
              1. Probed = IO (non-prefetchable) [size=128B]
              2. Configured = 0xc000
            1:
              0. Initial = 0xfebc0000
              1. Probed = MMIO (non-prefetchable) [size=4K]
              2. Configured = 0xfebc0000
            4:
              0. Initial = 0xfebe8000
              1. Probed = MMIO (64-bit, prefetchable) [size=16K]
              2. Configured = 0xfebe8000
          Interrupts:
            Allocated = 2
            Base Vector = 44
            Mode = MSI-X
[...]
        00:1f.3:
          BARs:
            4:
              0. Initial = 0x700
              1. Probed = IO (non-prefetchable) [size=64B]
              2. Configured = 0x700
```

## Tools

* [`//src/devices/pci/bin/lspci:lspci`](https://cs.opensource.google/fuchsia/fuchsia/+/main:src/devices/pci/bin/lspci/BUILD.gn;l=36)
  * Provides `lspci`, a tool to inspect and dump the state of PCI buses and devices in the system.
