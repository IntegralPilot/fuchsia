# Developing drivers for new display hardware

Writing a new driver for a piece of hardware is informally referred to as
"bringup". (Under a more strict interpretation, the term "bringup" only applies
to the first driver ever written for the hardware, because that driver serves to
validate the hardware itself. We do not follow this strict interpretation.)

[RFC-0275: Panel drivers][panel-drivers-rfc] recommends writing separate drivers
for the display engine hardware in the SoC and the display panel. For clarity,
this guide distinguishes between display engine driver and panel driver
functionality where applicable. However, currently the fastest path for early
development is to have a single driver cover the entire display path.

## Principles {:#principles}

The following principles are obvious in retrospect, but can be easy to forget
while you're in a rush or excited to bring up the hardware.

1. Collect and organize all available information on the hardware. Finding
   facts is much faster than re-discovering them.
2. Break down driver development into minimal increments that can be validated.
   Creativity here is highly rewarded.

## Working with information sources {:#information-sources}

Collect all the information sources you can access. Strive to have multiple
information sources covering each aspect of the hardware, so you can recover
from errors and omissions in any source. See
[Referencing information sources][referencing] for the recommended formats to
record references, citations, and aliases in Fuchsia display drivers.

You may have to search for information sources multiple times if you uncover
new hardware functional units on your hardware's display path. [PMICs][pmic]
tend to be left out of high-level diagrams. Use the
[display hardware overview][display-hardware-overview] as a checklist of
hardware to look for.

### Vendor documentation {:#vendor-docs}

Look for the following types of information.

* **Theory of operation**: A system architecture view (high-level descriptions
  of the hardware's functional units and how they are connected), and a
  functional view (supported features and operations, high-level description of
  how the functional units work together to implement the features and
  operations).

* **Hardware interface**: Register definitions (MMIO offsets, value encoding
  including any bit fields, semantics) and shared memory protocols (example:
  the layout of command buffers submitted to hardware).

* **Operational guidance**: Algorithms (sequences of operations) for the
  operations supported by the hardware. Example operations: powering off,
  displaying a layer of pixel data.

* **Errata** / **known issues** / **software workarounds**: Hardware bugs that
  the driver is expected to compensate for, and any recommended workarounds.

### Reference code {:#reference-code}

Look for source code for the following.

* **Drivers for other environments**: Drivers for embedded environments and
  bootloaders tend to be easier to analyze, and are more likely to contain
  minimal sequences of operations needed to work with the hardware. Drivers for
  full-featured operating systems are more complex, but are also more likely to
  cover the entire surface area of the hardware.

* **Your system's bootloader**: This bootloader's source code also describes the
  state of the display hardware when the bootloader hands off control to the
  operating system, which ultimately starts your driver.

* **Bringup code**: Unfortunately, hardware validation tests are rarely
  available to driver developers.

Reference code is likely to convey the hardware interface, operational guidance,
and errata more precisely than vendor documentation. (Programming languages
require precision, whereas natural language allows ambiguity.)

Reference code does not usually describe the theory of operation. You'll have to
rely on vendor documentation or infer the theory of operation from the code.

When the reference code conflicts with the vendor documentation, it is safer to
bet on the reference code. The code may be buggy, but you can verify that it
drives the hardware through the supported operations. When possible, carry out
experiments that check all the conflicting possibilities, and record the
experiment results in comments.

#### Traces {:#reference-code-traces}

Hardware access traces are a useful view for understanding complex reference
code.

Trace every interaction with the target hardware, such as MMIO register
accesses, configuration bus I/O operations, and requests to lower-level drivers
(example: voltage regulator configuration via a PMIC driver). Include all the
available information, such as the MMIO register address and the value being
read or written.

Augment the trace with execution flow information for the target hardware's
reference code. Scope the tracing to the code driving your hardware, to keep the
trace size manageable. Trace function entries, and include the parameters
(argument values). Trace function exits, and include the return values.

Trace output format tips:

* Prefix each trace line with a unique tag, such as `@display_trace`. The tags
  identify your trace lines in logs shared by multiple modules. The tags also
  identify the tracing logic you're adding to the reference code.

* Include the file name and function name or class + method name. These are
  most valuable, because they are not changed by adding tracing logic. Use
  macros (`__FILE__` and `__FUNCTION__` in C or C++, `file!()` and
  `module_path!()` in Rust) if they work as expected.

* Include the line number, and use it as a hint. Rely on macros (`__LINE__` in
  C/C++, `line!()` in Rust). Do not hard-code a line number that would become
  inaccurate as you iterate on the tracing logic. Keep in mind that the line
  numbers will not match the original reference code, but are still useful hints
  when combined with function entries.

* Make the format easy to parse programmatically.

### Hardware definition databases {:#register-databases}

Machine-readable register definitions, such as a [SystemRDL][system-rdl]
database, are incredibly valuable for new driver development. Unfortunately,
it's rare that the databases exist and are available to driver developers.

Kick-start early driver development by generating the initial register
definitions and tests from the register database. Revise the generated
definitions to bring them in compliance with
[our principles][register-definitions].

As development progresses, use the register database to resolve conflicts
between information sources. Register databases are typically generated from the
[Hardware Description Language (HDL)][hdl] source, so they are as close as
possible to the ground truth.

### Hardware inventory {:#hardware-inventory}

While working through information sources, build an inventory of all the
hardware functional units that you'll have to drive, and all the resources that
you'll have to expose to the display and panel drivers.

Keep an eye out for the following:

* **[MMIO][mmio] regions**: Cover all the registers in the [SoC][soc] display
  functional units. Most display engine drivers need multiple MMIO regions.
  Counterexample: Intel display engines conveniently place all display blocks in
  one MMIO region.

* **Interrupts**: Prioritize the interrupt for the Tearing Effect (TE) or
  [VSync][vsync] signal. Usually comes from the display engine. In rare cases,
  a dedicated [GPIO][gpio] pin receives the TE / VSync signal directly from the
  panel.

* **[GPIO][gpio] pins**: Board-dependent. Look for the Hot Plug Detect (HPD) pin
  for [DisplayPort (DP)][dp] and [HDMI][hdmi] displays. Look for the reset pin
  for [DSI][dsi] displays.

* **DDIC configuration bus**: Typically accessed via the SoC's I/O controllers.
  Rarely exposed via a functional unit in the display engine (example: Intel
  GMBUS). HDMI DDICs follow the [DDC][ddc] standard, and DisplayPort DDICs
  follow the AUX standard. DSI DDICs typically use DSI packets for
  configuration, but may (rarely) have an [I2C][i2c] or [SPI][spi] configuration
  bus.

* **Display engine [BTI][bti]**: If your display engine has an [IOMMU][iommu]
  supported by Zircon, you'll need a BTI that pins VMOs to the IOMMU's address
  space. Otherwise, you'll use a BTI for the CPU's [MMU][mmu], and your display
  engine driver will either program the engine's IOMMU directly (example: the
  Intel display engine's [GTT][intel-gtt]), or use the CPU's physical address
  space.

* **Power domains, voltage regulators, clocks**: Typically exposed as FIDL
  resources by a driver for the SoC or for the main PMIC. If your display engine
  includes functional blocks equivalent to a PMIC, your driver will configure
  these resources directly. Example: Intel display engine drivers configure
  all the resource types mentioned here.

* **Display PMIC resources**: Discrete display PMICs are owned by panel drivers.
  PMIC-equivalent functionality in the SoC display engine is owned by the
  display engine driver. Discrete display PMICs may be configured by the SoC via
  an I2C, SPI, or [SPMI][spmi] bus, or may be configured by the DDIC. PMICs may
  use other resources, such as a GPIO reset pin, or a voltage regulator.

## Development breakdown {:#dev-breakdown}

### Route resources to drivers {:#route-resources}

Start by exposing everything in the hardware resource inventory to the driver.
In addition, expose a **[Sysmem][sysmem] allocator client**, so the driver can
convey the display engine's constraints and obtain pixel data VMOs.

#### Validation {:#resource-validation}

All methods below assume that the corresponding functional units are powered on,
which is covered in the next section.

* **MMIO ranges**: Look for version or hardware ID registers in each functional
  unit. Log their values, and check that they match the documented values. If
  there are no version registers, use any register with a value that you can
  predict, and that does not look like a bus error (all bits set to 0 or 1).

* **Interrupts**: Early driver development tends to use polling. After all the
  display hardware is configured correctly, use an interrupt handler that solely
  logs the interrupt's arrival.

* **GPIO pins and configuration buses**: Use
  [hardware testing tools][hw-testing] such as `gpioutil` and `i2cutil`. If the
  bootloader leaves the panel on, pulsing reset GPIOs will be visible. Look for
  configuration commands that read version or hardware ID registers.

### Power on and clock the hardware {:#power-up}

In early development, err on the side of powering on everything. After the
initial bringup, fine-tune the configuration to reduce power consumption.

* Set all the voltage regulators to nominal voltage.
* Enable and/or [ungate][power-gating] all the power domains (rails).
* [Ungate][clock-gating] all the clocks, set to the highest operating frequency.

If the display hardware includes PMIC functionality:

* Adopt the recommended [MUX][mux] configuration for all clock branches.
* Set [PLLs][pll] to the highest supported frequency and lock them.

Prioritize the display functional units on the SoC. Accessing the registers of a
powered-off functional unit can stall the SoC's interconnect fabric /
[NoC][noc], which leads to the entire system locking up or rebooting, depending
on hardware [watchdog timer (WDT)][watchdog] availability.

Recovering from a locked-up system is more difficult than recovering from a
reboot cycle. For this reason, Zircon bringup prioritizes WDT enablement. If you
encounter lockups, it may be easier to help out with WDT enablement before
proceeding with debugging.

**Debugging SoC reboots**: Before any hardware interaction (example: MMIO
register access), log the interaction and sleep for a few hundred milliseconds
to allow the serial logs to drain to your development computer. If the last
interaction before the SoC reboots involves a new functional unit, the most
likely causes are that the unit is not powered on, or its resources are routed
incorrectly.

#### Validation {:#power-up-validation}

Search for status registers that confirm the power state changes took effect.

* **Clocks**: Look for a status bit indicating if the clock is running. Look for
  diagnostics hardware that can be configured to measure the clock's frequency.
* **PLLs**: Look for a status bit indicating if the PLL is locked, and for the
  maximum time it takes the PLL to lock after it is reconfigured.
* **Power domains**: Look for a status bit indicating if the domain is powered.

When the status signals exist, strongly consider stopping the hardware
initialization sequence on power errors. Ignoring errors risks locking up the
SoC.

### Backend (panel connection) {:#backend}

The display engine backend includes the PHY and digital display interface or
host controller. For a high-level overview of the display path, DDIC, and
backend/frontend split, see the
[Display hardware overview][display-hardware-overview].

On some systems, the bootloader leaves the backend initialized. If that's the
case, focus on the frontend, which is covered in the next section. You can come
back to the backend to look for power savings after completing the initial
driver version.

#### Validation {:#backend-validation}

Prioritize implementing functionality for reading the panel ID bytes from the
DDIC (examples: [DCS][dcs] ID bytes, [EDID][edid]), and compare the read ID with
the documented value. This check validates the backend configuration, at least
for the low-speed connection used for DDIC configuration.

DSI DDICs support an Inverted mode, which is entered using a (relatively) short
and simple command. The command does not require bi-directional communication
(unlike reading the panel ID bytes), and has a very visible effect on the
display panel. Look for similar testing commands for your DDIC.

Some display engine backends have a Test Pattern Generator (TPG), which replaces
the pixel data coming from the frontend with a fixed test pattern. If your
hardware has a backend TPG, use it to validate the high-speed connection used
to transfer pixel data without depending on correct frontend configuration.

**Caveat to TPG-based validation**: Some validation processes only rely on TPGs
in early stages of testing, so the TPG hardware may not be usable on production
hardware. If you have access to a reference driver that enables the TPG, check
that it works on production hardware.

### Frontend (pixel data fetching and compositing) {:#frontend}

In early development, prioritize getting single-layer configurations working.

#### Validation {:#frontend-validation}

Search each functional unit's documentation for testing features such as TPGs
and Solid Color Fill. These features discard the functional unit's input and
generate a deterministic output, reducing the amount of code under consideration
when investigating a driver problem.

If multiple functional units have testing features, start with the unit closest
to the backend and work your way backwards towards the beginning of the frontend
pipeline.

If the same functional unit has both Solid Color Fill and a TPG, use the Solid
Color Fill. See the TPG reliability caveat in the backend validation section.

[The `display-tool` README][display-tool-readme] covers more advanced validation
steps.

[bti]: /docs/reference/kernel_objects/bus_transaction_initiator.md
[clock-gating]: https://en.wikipedia.org/wiki/Clock_gating
[dcs]: https://www.mipi.org/specifications/display-command-set
[ddc]: https://en.wikipedia.org/wiki/Display_Data_Channel
[display-hardware-overview]: /docs/development/drivers/driver_guides/display/hardware.md
[display-tool-readme]: /src/graphics/display/bin/display-tool/README.md
[dp]: https://en.wikipedia.org/wiki/DisplayPort
[dsi]: https://en.wikipedia.org/wiki/Display_Serial_Interface
[edid]: https://en.wikipedia.org/wiki/Extended_Display_Identification_Data
[gpio]: https://en.wikipedia.org/wiki/General-purpose_input/output
[hdl]: https://en.wikipedia.org/wiki/Hardware_description_language
[hdmi]: https://en.wikipedia.org/wiki/HDMI
[hw-testing]: /docs/development/testing/hardware/guide.md
[i2c]: https://en.wikipedia.org/wiki/I2C
[intel-gtt]: https://en.wikipedia.org/wiki/Graphics_address_remapping_table
[iommu]: https://en.wikipedia.org/wiki/Input%E2%80%93output_memory_management_unit
[mmio]: https://en.wikipedia.org/wiki/Memory-mapped_I/O_and_port-mapped_I/O
[mmu]: https://en.wikipedia.org/wiki/Memory_management_unit
[mux]: https://en.wikipedia.org/wiki/Multiplexer
[noc]: https://en.wikipedia.org/wiki/Network_on_a_chip
[panel-drivers-rfc]: /docs/contribute/governance/rfcs/0275_panel_drivers.md
[pll]: https://en.wikipedia.org/wiki/Phase-locked_loop
[pmic]: https://en.wikipedia.org/wiki/Power_management_integrated_circuit
[power-gating]: https://en.wikipedia.org/wiki/Power_gating
[referencing]: /docs/development/drivers/driver_guides/display/referencing.md
[register-definitions]: /docs/development/drivers/driver_guides/display/register-definitions.md
[soc]: https://en.wikipedia.org/wiki/System_on_a_chip
[spi]: https://en.wikipedia.org/wiki/Serial_Peripheral_Interface
[spmi]: https://en.wikipedia.org/wiki/System_Power_Management_Interface
[sysmem]: /docs/development/graphics/sysmem/concepts/sysmem.md
[system-rdl]: https://en.wikipedia.org/wiki/SystemRDL
[vsync]: https://en.wikipedia.org/wiki/Analog_television#Vertical_synchronization
[watchdog]: /docs/concepts/kernel/watchdog.md
