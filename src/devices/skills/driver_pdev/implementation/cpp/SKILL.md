---
name: use-pdev-dfv2
description: >
  Acquire hardware resources via the Platform Device (pdev) protocol in a C++
  DFv2 driver. Use when a driver for a devicetree/platform device must connect
  to fuchsia.hardware.platform.device/Service and construct an fdf::PDev to
  map MMIO (MapMmio), obtain an interrupt/IRQ (GetInterrupt), or get a BTI
  (GetBti), plus the matching use entry in .cml.
---

# Using Platform Device (pdev) in DFv2

## Dependencies

**GN:**
```gn
deps = [
  "//sdk/lib/driver/platform-device/cpp", # For fdf::PDev
]
```

**Bazel:**
```bazel
deps = [
  "@fuchsia_sdk//pkg/driver_platform_device_cpp", # For fdf::PDev
]
```

## Implementation Steps

### 1. Component Manifest (.cml) Update

Declare that the driver uses the service in its `.cml` file:

```cml
    use: [
        { service: "fuchsia.hardware.platform.device.Service" },
    ],
```

### 2. Code Implementation

Include the header:
```cpp
#include <lib/driver/platform-device/cpp/pdev.h>
```

To connect to `pdev` in your `Start()` method:
```cpp
zx::result pdev_client = incoming()->Connect<fuchsia_hardware_platform_device::Service::Device>();
if (pdev_client.is_error()) {
  fdf::error("Failed to connect to pdev: {}", pdev_client.status_string());
  return pdev_client.take_error();
}
fdf::PDev pdev(std::move(pdev_client.value()));
```

### Common Operations

* **Map MMIO**:
  ```cpp
  zx::result mmio = pdev.MapMmio(0);
  ```
* **Get Interrupt**:
  ```cpp
  zx::result irq = pdev.GetInterrupt(0);
  ```
* **Get BTI**:
  ```cpp
  zx::result bti = pdev.GetBti(0);
  ```

## Further Reading

* For more information on FIDL usage, see the [Driver FIDL Usage Implementation
  Skill
  (C++)](/src/devices/skills/driver_fidl/client/implementation/cpp/SKILL.md).
