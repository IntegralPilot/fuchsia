# GPIO initialization

Caution: This page may contain information that is specific to the legacy
version of the driver framework (DFv1).

Some drivers depend on a static GPIO pin or pin mux configuration that is
board-specific. In these cases it is often desirable to keep the GPIO
configuration details in the board driver to ensure that the dependent drivers
remain portable. This can be accomplished by having the board driver pass
metadata with a list of initialization steps to the GPIO driver. See below for
an example.

## Configuring GPIO metadata in the board driver

Consider a case where an SDMMC driver requires pins to be configured with a
certain alt function and drive strength. To do this, the board driver would
create a list of `fuchsia.hardware.gpioimpl.InitStep` objects specifying the
GPIO protocol calls to make, and the GPIO index to make them on:

```cpp
#include <fidl/fuchsia.hardware.gpioimpl/cpp/wire.h>

// Helper lambda to simplify the following code.
auto sdmmc_gpio = [&](uint64_t alt_function, uint64_t drive_strength_ua) {
  return fuchsia_hardware_gpioimpl::wire::InitOptions::Builder(arena)
      .alt_function(alt_function)
      .drive_strength_ua(drive_strength_ua)
      .Build();
};

const fuchsia_hardware_gpioimpl::wire::InitStep init_steps[] = {
    {SDMMC_GPIO_0, sdmmc_gpio(SDMMC_GPIO_0_ALT_FUNCTION, 4000)},
    {SDMMC_GPIO_1, sdmmc_gpio(SDMMC_GPIO_1_ALT_FUNCTION, 4000)},
    {SDMMC_GPIO_2, sdmmc_gpio(SDMMC_GPIO_2_ALT_FUNCTION, 4000)},
    {SDMMC_GPIO_3, sdmmc_gpio(SDMMC_GPIO_3_ALT_FUNCTION, 4000)},
    {SDMMC_GPIO_4, sdmmc_gpio(SDMMC_GPIO_4_ALT_FUNCTION, 4000)},
    {SDMMC_GPIO_5, sdmmc_gpio(SDMMC_GPIO_5_ALT_FUNCTION, 4000)},
};
```

This list is then given to the GPIO driver as metadata:

```cpp
{% verbatim %}
fuchsia_hardware_gpioimpl::wire::InitMetadata metadata;
metadata.steps = fidl::VectorView<fuchsia_hardware_gpioimpl::wire::InitStep>::FromExternal(
    init_steps, std::size(init_steps));

fit::result encoded = fidl::Persist(metadata);
if (!encoded.is_ok()) {
  return encoded.error_value().status();
}

const std::vector<fpbus::Metadata> gpio_metadata{
    {{
        .type = DEVICE_METADATA_GPIO_INIT_STEPS,
        .data = std::move(encoded.value()),
    }},
    // Other metadata goes here.
};

const fpbus::Node gpio_dev = []() {
  fpbus::Node dev = {};
  dev.name() = "gpio";
  dev.vid() = PDEV_VID_SOME_VENDOR;
  dev.pid() = PDEV_PID_SOME_PRODUCT;
  dev.did() = PDEV_DID_SOME_GPIO_DEVICE;
  dev.mmio() = gpio_mmios;
  dev.irq() = gpio_irqs;
  dev.metadata() = gpio_metadata;
  return dev;
}();

// Add the node here.
{% endverbatim %}
```

## Creating a dependency on the GPIO configuration with bind rules

Now that the GPIO driver is configuring these pins correctly, the SDMMC driver
must be made to depend on this configuration before binding. This is
accomplished by adding an additional fragment to the SDMMC device in its board
driver bind file:

```none
using fuchsia.gpio;

// Other nodes go here.

node "gpio-init" {
  fuchsia.BIND_INIT_STEP == fuchsia.gpio.BIND_INIT_STEP.GPIO;
  fuchsia.BIND_GPIO_CONTROLLER = 1;
}
```

The `fuchsia.gpio.BIND_INIT_STEP.GPIO` device is added by the GPIO driver after
it has finished processing the initialization steps. Any number of children can
bind to this device to ensure that their GPIOs have been configured before
starting.

## How the GPIO driver handles configuration errors

When the GPIO driver encounters an error while processing initialization steps,
it continues to set up the rest of the GPIOs, but does not add the init device.
This is to ensure that as many pins as possible are put into a known state, and
that drivers will not attempt to run with pins that are potentially
misconfigured. Similarly, no init device is added if the GPIO driver does not
have or cannot parse the initialization metadata.

## Recommended use cases

There are two cases where the use of GPIO init steps is recommended:

- A driver requires static board- or platform-specific GPIO configuration.

The pull-up/pull-down, alt function, and drive strength settings are typically
board- or platform-specific, so using GPIO init to configure them in the board
driver is preferred. Setting a GPIO output value this way is also recommended if
the value is not the same (or not required) on all boards.

- Multiple drivers depend on the static configuration of one GPIO.

If multiple drivers depend on the static configuration of one GPIO, then that
configuration should be done by GPIO init. This prevents multiple drivers from
having to make potentially conflicting calls to a single GPIO.

GPIO init steps should not be used for any configuration that will be changed by
the driver at runtime.

## GPIO initialization options

See the full list of initialization options and their explanations in the
[fuchsia.hardware.pinimpl](/sdk/fidl/fuchsia.hardware.pinimpl/metadata.fidl) FIDL
specification.
