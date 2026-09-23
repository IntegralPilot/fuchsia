# Registers overview

The [registers driver][registers-driver] should be used for registers that
may be accessed by multiple drivers.

## Concepts

Drivers often communicate with hardware via register-based interfaces.
Conceptually, registers are groups of 8/16/32/64 bits that can be read or
written via an address space. MMIO registers are accessed via the CPU's
memory address space, while I2C registers are accessed via an I2C bus.

Most hardware only supports atomic register writes. So, drivers that want
to change a few bits in a register must do so via read/modify/write
operations. For example, for a 32-bit register, if you were to change the
0-1st bits both to `0`, you would need to read all 32 bits of that register,
say `0xFFFFFFFF`, then write back to that register `0xFFFFFFFC`.

### Synchronization

Read/modify/write operations introduce the potential for data races. The
classic transaction example "withdraw $10 from a bank account" is
effectively a read/modify/write operation. See [Data Race](#data-race) in
the [Examples](#examples) section for a concrete illustration.

When a register is "exclusively owned" by a driver, the driver is expected
to synchronize its accesses to the register to avoid data races. (The
potential for races still exists, but it's outside the scope of the
register driver).

When a register is "shared" (may be accessed) by multiple drivers, we need
a global coordinator to avoid races. The register driver can act as this
global coordinator, when each driver only needs to access a disjoint subset
of the register's bits. The global coordinator will provide the needed
synchronization between reads/writes of a register.

### Isolation

The registers driver not only prevents races by providing synchronization,
but also provides isolation of bit fields. The registers driver splits
a register up into multiple resources and ensures that each driver only
has access to those bits. A driver may not accidentally read or write into
bits that it does not own.

### Theory of operation

In the [board driver][board-driver], a register
which is accessed by multiple drivers declares the different bit fields
that it wants to expose. The registers driver creates devices for each
of the bit fields. A driver that wants to access those bit fields needs
to bind to the corresponding device.

Within the registers driver, a lock for each register is used to ensure
that only one read/write can access the register at once. The interface of
the registers driver is defined by [registers-util.fidl][registers-util.fidl].

## How to use

Currently the [Reset Registers](#reset-registers) are the only registers
that are migrated to use the registers driver. This section will use them
as an example on how to use the registers driver.

1. Board driver changes

   Metadata format is declared in [metadata.fidl][metadata.fidl].

   In the board driver (i.e. [nelson-registers][nelson-registers]), make the
   following changes.

   a. MMIO

      Ensure that the registers driver has access to the MMIO. If not,
      add MMIO and corresponding MMIO index:

      ```c++ {:.devsite-disable-click-to-copy}
      enum MmioMetadataIdx {
        kResetMmio,

        kMmioCount,
      };

      static const std::vector<fpbus::Mmio> registers_mmios{
          {
            {
              .base = A311D_RESET_BASE,
              .length = A311D_RESET_LENGTH,
            },
          },
      };
      ```

      Note that the `MmioMetadataIndex` must correspond to the MMIO's
      index in `registers_mmios`.

   b. Declare bitfields

      The [`RegistersMetadataToFidl`][fidl_metadata::registers] helper
      function can be used to declare bitfields:

      ```c++ {:.devsite-disable-click-to-copy}
      {% verbatim %}
      zx::result metadata = fidl_metadata::registers::RegistersMetadataToFidl(kRegisters);
      if (!metadata.is_ok()) {
        zxlogf(ERROR, "Failed to convert registers to metadata %s", metadata.status_string());
        return metadata.error_value();
      }

      fit::result persisted_metadata = fidl::Persist(metadata.value());
      if (!persisted_metadata.is_ok()) {
        zxlogf(ERROR, "Failed to persist registers metadata: %s",
              persisted_metadata.error_value().FormatDescription().c_str());
        return persisted_metadata.error_value().status();
      }

      std::vector<fpbus::Metadata> registers_metadata{
          {{
              .id = fuchsia_hardware_registers::Metadata::kSerializableName,
              .data = std::move(persisted_metadata.value()),
          }},
      };
      {% endverbatim %}
      ```

      where the bitfields are defined in the `kRegisters` field:

      ```c++ {:.devsite-disable-click-to-copy}
      static const fidl_metadata::registers::Register<uint32_t> kRegisters[]{
          {
              .name = bind_fuchsia_amlogic_platform::NAME_REGISTER_USB_PHY_V2_RESET,
              .mmio_id = kResetMmio,
              .masks =
                  {
                      {
                          .value = aml_registers::USB_RESET1_REGISTER_UNKNOWN_1_MASK |
                                   aml_registers::USB_RESET1_REGISTER_UNKNOWN_2_MASK,
                          .mmio_offset = A311D_RESET1_REGISTER,
                      },
                      {
                          .value = aml_registers::USB_RESET1_LEVEL_MASK,
                          .mmio_offset = A311D_RESET1_LEVEL,
                      },
                  },
          },
          ...
      };
      ```

      * `name`: The unique name for each bitfield definition that is
        used to identify it during binding.
      * `mmio_id`: The ID corresponding to the MMIO that this bitfield
        is in reference to.
      * `masks`: A list of masks describing the specific bitfields that
        should be accessible through this register device.
        * `value`: The bitmask that is available to access, with `1`
          meaning accessible and `0` inaccessible. For example for a
          32-bit register, a bitmask may be `0xFFFF0000`, which means
          the higher 16 bits of the register are accessible through this
          register device and the lower 16 are not.
        * `mmio_offset`: The starting offset from the start of the MMIO
          identified by `mmio_id` that these bitfields define.
        * `count`: The number of register address this bitfield mask
          applies to following `mmio_offset`. Defaults to `1`.
        * `overlap_check_on`: If `true`, the registers driver will
          verify that these bitfields do not overlap with any other
          defined bitfields. Otherwise, the check is skipped. Defaults
          to `true`.

   The registers driver will create a device that serves [registers-util.fidl][registers-util.fidl]
   with bind properties:
   ``` {:.devsite-disable-click-to-copy}
   bind_fuchsia::NAME == name
   ```

1. Binding to the registers driver

   The driver that wishes to access these bitfields must then bind to
   the device created above.

1. Using the registers driver interface

   After successfully connecting to the registers device, say through
   `fidl::WireSyncClient<fuchsia_hardware_registers::Device> register`,
   you may call the corresponding FIDL read/write according to the FIDL
   interface defined in [registers-util.fidl][registers-util.fidl].
   E.g., for 32-bit registers,

   ```c++ {:.devsite-disable-click-to-copy}
   auto result =
       reset_register_->WriteRegister32(RESET1_LEVEL_OFFSET, aml_registers::USB_RESET1_LEVEL_MASK,
                                        aml_registers::USB_RESET1_LEVEL_MASK);
   if ((result.status() != ZX_OK) || result->is_error()) {
     zxlogf(ERROR, "Write failed\n");
     return ZX_ERR_INTERNAL;
   }
   ```

1. Enjoy isolated synchronized registers!

## Examples

### Data Race

Say we have a register that is accessed by both driver A and driver B.
Now driver A wants to write `b01` to bits 0-1 of the register and driver
B wants to write `b001` to bits 8-10 of the register. Depending on timing
driver A may read the original value of the register, say `0xFFFFFFFF` and
driver B may also read that same value. Driver A then writes to bits 0-1
and puts into memory `0xFFFFFFFD`. Driver B then writes to memory
`0xFFFFF9FF`. In this sequence of events, the register now holds the value
of `0xFFFFF9FF`, which driver B had written last. However, the next time
driver A comes to read bits 0-1 of this register, it will not get the
value it expects as it was previously written.

### AMLogic SoCs

This section gives some examples of where the registers driver should and
should not be used. The registers driver definitely should be used when it
is needed for synchronization between multiple drivers, and may or may not be
used only for isolation of resources. Note that the following is not a complete
list of shared registers in the AMLogic SoCs.

#### Reset Registers

AMLogic SoCs have the reset functionality for various hardware units
concentrated in a few 32-bit registers. This is exemplified by
`RESET1_REGISTER` in [S905D3][S905D3] Table 6-186. For example, USB reset is
controlled by bit 2 and SD_EMMC by bits 12-14. The hardware design forces
us to share `RESET1_REGISTER` among multiple drivers including the EMMC
driver and the USB driver.

See [nelson-registers][nelson-registers] for the board file changes made for reset
registers and [nelson-usb][nelson-usb] - `aml_usb_phy` device for adding reset
register fragments. `AmlUsbPhy::InitPhy()` of [aml-usb-phy][aml-usb-phy] uses
the FIDL client to write to the reset register.

#### Power Registers

Similar to the reset registers described above, `AO_RTI_GEN_PWR_SLEEP0`
([S905D3][S905D3] Table 6-17) and `AO_RTI_GEN_PWR_ISO0` ([S905D3][S905D3] Table
6-18) are shared by multiple drivers, including display, USB, and ML, and should
be managed by the registers driver because they are writable and need
coordination. This migration is currently in progress.

On the other hand, although `AO_RTI_GEN_PWR_ACK0` ([S905D3][S905D3] Table 6-19)
will be shared by multiple drivers (PCIE, USB, display, etc.), it is readonly.
Data from this register can be accessed concurrently without any risk of races,
so it is not required to use the registers driver for synchronization. If
desired, we may still use the registers driver for isolation of resources.

<!-- Reference links -->

[aml-usb-phy]: /src/devices/usb/drivers/aml-usb-phy/aml-usb-phy.cc
[board-driver]: /docs/glossary/README.md#board-driver
[fidl_metadata::registers]: /src/devices/lib/fidl-metadata/registers.h
[metadata.fidl]: /sdk/fidl/fuchsia.hardware.registers/metadata.fidl
[registers-driver]: /src/devices/registers/drivers/registers
[registers-util.fidl]: /sdk/fidl/fuchsia.hardware.registers/register-util.fidl
[S905D3]: https://dl.khadas.com/products/vim3l/datasheet/s905d3_datasheet_0.2_wesion.pdf
[nelson-registers]: /src/devices/board/drivers/nelson/nelson-registers.cc
[nelson-usb]: /src/devices/board/drivers/nelson/nelson-usb.cc
