// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_REGISTERS_GMBUS_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_REGISTERS_GMBUS_H_

#include <lib/driver/logging/cpp/logger.h>
#include <zircon/assert.h>

#include <optional>
#include <span>

#include <hwreg/bitfields.h>

#include "src/graphics/display/drivers/intel-display/i2c/gmbus-gpio.h"

namespace registers {

// GMBUS Configuration Registers
//
// GMBUS is a (confusingly named) piece of hardware that implements of a subset
// of the I2C protocol over the GPIO pins. Configuring GMBUS is significantly
// more efficient than implementing I2C directly via the low-level GPIO
// interface exposed by the `GpioPinPairControl` (GPIO_CTL) registers (also
// known as "bit banging"). This makes GMBUS the preferred way to act as the
// controller device, for all I2C-based protocols that meet the GMBUS
// constraints. In particular, the DDC (Display Data Channel) and E-DDC
// (Extended Display Data Channel protocol can be implemented using GMBUS.
//
// Note:
//
// The I2C specs ("I2C (Inter-IC) bus specification and user manual") and Intel
// Graphics Programmers' Reference Manual may use different sets of terms for
// the bus devices. These terms may be used interchangeably:
// - The canonical terms, introduced by revision 7.0 of the I2C spec, are
//   "controller" for the device that initiates I2C transactions and drives the
//   clock pin, and "target" for the device that replies to transactions.
// - Some Intel documentation (including the Tiger Lake PRM) and VESA standards
// use "I2C primary" or "source device" to mean "controller", and "I2C
// secondary" and "sink device" to mean "target.
// - Older I2C specifications, Intel documentation (including the Kaby Lake
// PRM), and VESA standards may use the
//   non-inclusive terms "master" for "controller", and "slave" for "target".
//
// The following terms may be used interchangeably within documentations of
// GMBUS- and GPIO-related register definitions and code:
// - Software / Driver / Display driver (us).
// - Hardware / Source Device / I2C Controller / GMBUS Controller.

// GMBUS0
// (Graphic Management Bus Configuration Register 0 -- Clock / Port Select)
//
// This register controls the clock rate of the serial bus and selects the
// device pin pair the controller is connected to. This register must be
// configured before the first data valid bit is set.
//
// Some of this register's reserved fields are not MBZ (must be zero). So, the
// register can only be updated safely via read-modify-write operations.
//
// Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev 2.0 Part 1, Page 1020
// Kaby Lake: IHD-OS-KBL-Vol 2c-1.17 Part 1, Page 728
// Skylake: IHD-OS-SKL-Vol 2c-05.16 Part 1, Page 723
class GMBusClockPortSelect : public hwreg::RegisterBase<GMBusClockPortSelect, uint32_t> {
 public:
  enum class BusRate {
    // At this rate, the I2C bus will run in I2C Standard Mode.
    //
    // This rate is supported by all Display Engine platforms this driver
    // supports.
    k100Khz = 0b000,

    // At this rate, the I2C bus will run in I2C Standard Mode.
    //
    // This rate is supported by all Display Engine platforms this driver
    // supports.
    k50Khz = 0b001,

    // At this rate, the I2C bus will run in I2C Fast Mode.
    //
    // This value is mentioned in some old generation Intel Display Engine
    // documentation (e.g. Ivy Bridge IHD-OS-V3 Pt 4-05 12), but it's not in
    // any of the platforms supported by this driver.
    k400KhzUnsupported = 0b010,

    // At this rate, the I2C bus will run in I2C Fast Mode Plus.
    //
    // This value is mentioned in some old generation Intel Display Engine
    // documentation (e.g. Ivy Bridge IHD-OS-V3 Pt 4-05 12), but it's not in
    // any of the platforms supported by this driver.
    k1MhzUnsupported = 0b011,
  };

  DEF_RSVDZ_FIELD(31, 12);

  // This selects the clock rate of the serial bus, i.e. the I2C protocol clock
  // when drivers use GMBUS for data transfer. It defines the AC timing
  // parameters used for different bus rates / I2C bus mode.
  //
  // This field must only be changed when the `is_active` field in
  // `GMBusControllerStatus` (GMBUS2) register is false, i.e. the GMBUS is idle.
  //
  // Note that this only changes the clock rate when transferring data using
  // GMBUS protocol. If drivers implements software-based bit banging using the
  // raw GPIO pins, they don't need to follow the bus rate.
  DEF_ENUM_FIELD(BusRate, 10, 8, bus_rate_select);

  // This field overrides the `total_byte_count` field on `GMBusCommand`
  // (GMBUS1) register. When this bit is enabled, the device will allow burst
  // reads greater than 511 bytes.
  //
  // On Tiger Lake, this feature is supported.
  // On Skylake, this feature is not supported and the bit must be zero.
  // The underlying feature is not implemented on some PCH chipsets used with
  // Kaby Lake display engines. The models are listed in IHD-OS-KBL-Vol 12-1.17
  // section "Sequence for GMBUS Burst Reads Greater Than 511 Bytes", page 199.
  DEF_BIT(6, byte_count_overridden);

  // In Intel Display Engine, every DDI is allocated a pair of numbered GPIO
  // pins for I2C / GMBUS communication.
  //
  // This field selects a GMBUS pin pair for use in the GMBUS communication.
  //
  // The DDI <-> pin pair mapping varies by platform. The mapping is available
  // at: Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev 2.0 Part 1, Page 1020 Kaby Lake:
  // IHD-OS-KBL-Vol 2c-1.17 Part 1, Page 728 Skylake: IHD-OS-SKL-Vol 2c-05.16
  // Part 1, Page 723
  //
  // Note that the GMBUS pin pair ID may not be equal to the GPIO pin pair ID.
  //
  // Note: On Skylake and Kaby Lake, only bits 2:0 are defined for this field
  // but bits 4:3 are reserved as "must be zero"; for compatibility we use the
  // same field definition across platforms, but the highest two bits on Skylake
  // and Kaby Lake will always be zero.
  DEF_FIELD(4, 0, pin_pair_select);

  // Helper to select typed `pin_pair` for GMBUS communication.
  GMBusClockPortSelect& SetPinPair(const intel_display::GMBusPinPair& pin_pair) {
    return set_pin_pair_select(pin_pair.number());
  }

  static auto Get() { return hwreg::RegisterAddr<GMBusClockPortSelect>(0xc5100); }
};

// GMBUS1
// (Graphic Management Bus Configuration Register 1 -- Command / Status)
//
// This register lets the software (driver) indicate to the GMBUS controller
// the target device address, register index and indicate when the data write
// is complete.
//
// All reserved bits in this register are MBZ (must be zero). So, the register
// can be safely updated without reading it first.
//
// Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev 2.0 Part 1, Page 1022
// Kaby Lake: IHD-OS-KBL-Vol 2c-1.17 Part 1, Page 730
// Skylake: IHD-OS-SKL-Vol 2c-05.16 Part 1, Page 724
class GMBusCommand : public hwreg::RegisterBase<GMBusCommand, uint32_t> {
 public:
  // This bit is used to clear error flags and resets the controller status.
  //
  // For normal bus operation, this bit must be set to 0.
  //
  // To perform a GMBUS reset,
  // - Software transitions this bit 0->1 to start the hardware reset sequence.
  //   The GMBUS configuration registers become write-protected.
  // - Hardware sets the `is_ready` bit in the `GMBusControllerStatus` (GMBUS2)
  //   register to 1 when the reset is complete.
  // - Software transitions this bit 1->0 to remove the write protection from
  //   the GMBUS configuration registers.
  // - Hardware sets the `is_ready` bit back to 0 and restores normal operation.
  //   GMBUS can be used.
  DEF_BIT(31, software_clear_interrupt);

  // Trigger the start of a GMBUS transfer cycle when set to 1.
  //
  // When `is_ready` in `GMBusControllerStatus` is true and `software_ready`
  // is 0, the driver can request the GMBUS controller to start handshaking
  // and data transfer by setting `software_ready` to 1.
  //
  // Then the `controller_is_ready` bit in `GMBusControllerStatus` (GMBUS2) register will be
  // de-asserted until GMBUS finishes reading / writing the data register (GMBUS3) or an active
  // GMBUS cycle is terminated. That will also set `software_ready` bit to 0.
  //
  // For each transfer, the driver only needs to set `software_ready` bit to 1
  // once and should not set it again once it's de-asserted.
  DEF_BIT(30, software_ready);

  // If true, target stall status bit in `GMBusControllerStatus` will be updated
  // and interrupts will be generated when corresponding mask bit is enabled,
  // when the target device stalls the acknowledgement bit longer than the time
  // limit specified (in VESA DDC/CI standard, the required time limit is 2
  // milliseconds).
  DEF_BIT(29, enable_timeout);

  // If true, an I2C STOP signal will be transmitted at the current GMBUS cycle
  // to terminate the transaction.
  //
  // Setting this to false while setting `wait_state_enabled` to true allows the
  // driver to continue data read / write by letting controller issue a RESTART
  // signal to start a new data cycle after the current GMBUS cycle completes.
  DEF_BIT(27, stop_generated);

  // If true, the GMBUS cycle contains a special index write transaction.
  //
  // When this field is true, the GMBUS controller uses the I2C combined format
  // to perform a 1-byte or 2-byte index write transaction, immediately followed
  // (via RESTART) by a second transaction in the direction indicated by
  // `is_read_transaction`. In other words, the main transaction is preceded
  // by a short (1-byte / 2-byte) write transaction.
  //
  // This operation matches the VESA E-DDC (Enhanced Display Data Channel)
  // specification for reading EDID / DisplayID contents, where the E-DDC
  // "start address" / "word offset" (0 or 128) is a 1-byte index.
  //
  // If this field is true, `wait_state_enabled` must also be true.
  DEF_BIT(26, index_transaction_used);

  // If true, the GMBUS controller may enter the wait state after the cycle.
  //
  // If this field is true and `stop_generated` is false, the GMBUS controller
  // will enter the wait state after completing the GMBUS cycle, waiting for
  // the driver to read / write the GMBusData register with new data bytes.
  //
  // During this state, the GMBUS controller pulls the I2C clock line low to
  // hold incoming transactions from target devices.
  DEF_BIT(25, wait_state_enabled);

  // Total number of bytes to be transferred (read / written) during the DATA
  // phase of the GMBUS cycle.
  //
  // The GMBUS controller is expected to read / write `total_byte_count` from
  // the bus, but this may be prematurely terminated by the driver (by
  // triggering a STOP cycle) if needed (for example, if the display engine
  // receives a NACK from the display device).
  //
  // This field must not be changed once a cycle transaction has started.
  // The value of this field must not be zero.
  DEF_FIELD(24, 16, total_byte_count);

  // When `index_transaction_used` is set, the index byte in this field will be
  // written to the `target_address` before the DATA read / write phase.
  //
  // This field is in effect only when `index_transaction_used` is set, and
  // `two_byte_index_enable` in GMBusTwoByteIndex (GMBUS5) is disabled.
  DEF_FIELD(15, 8, target_index);

  // 7-bit GMBUS target address (SADDR). This is the target address used in I2C
  // protocol when GMBUS generates bus cycles.
  //
  // On some documents (including VESA E-DDC and DDC/CI, and Intel Programmer's
  // Reference Manuals), an 8-bit address is used including this field and the
  // `is_read_transaction` bit.
  //
  // Some of the `target_address` / `is_read_transaction` combinations are
  // reserved by I2C and recognized by GMBUS controller for special purposes,
  // though the VESA DDC/CI protocol doesn't use or prohibits them.
  DEF_FIELD(7, 1, target_address);

  // True if the new GMBUS cycle to generate will read data from target device;
  // otherwise the new cycle will write data to target device.
  //
  // On some documents, this bit together with `target_address` are combined as
  // an 8-bit address for target addressing.
  DEF_BIT(0, is_read_transaction);

  static auto Get() { return hwreg::RegisterAddr<GMBusCommand>(0xc5104); }
};

// GMBUS2
// (Graphic Management Bus Configuration Register 2 -- Status)
//
// This register contains controller (hardware) status bits indicating the
// current controller state and readiness for GMBUS cycles.
//
// Besides `software_lock_acquired`, all the other bits are read-only in this
// register. `software_lock_acquired` is write-protected when
// `software_clear_interrupt` in `GMBusCommand` (GMBUS1) register is enabled.
//
// Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev 2.0 Part 1, Page 1025
// Kaby Lake: IHD-OS-KBL-Vol 2c-1.17 Part 1, Page 733
// Skylake: IHD-OS-SKL-Vol 2c-05.16 Part 1, Page 727
class GMBusControllerStatus : public hwreg::RegisterBase<GMBusControllerStatus, uint32_t> {
 public:
  DEF_RSVDZ_FIELD(31, 16);

  // This bit has NO effect on the hardware, and is only used as an indicator
  // for software to query and declare the usage of the GMBUS controller.
  //
  // Drivers can poll and wait until `software_lock_acquired` becomes false
  // before using the GMBUS controller, and set this bit (grab the software
  // lock) so that other threads won't use it at the same time.
  //
  // This bit is write-protected when `software_clear_interrupt` in
  // `GMBusCommand` (GMBUS1) register is enabled.
  DEF_BIT(15, software_lock_acquired);

  // True when the controller is in WAIT state after data read / write.
  //
  // Once this bit is true, the driver can set `GMBusCommand` register to
  // generate a STOP cycle or continue reading / writing, which will de-assert
  // this bit.
  DEF_BIT(14, is_waiting);

  // True when the target device has stalled the target device acknowledgement
  // beyond the time limit.
  //
  // This bit is only meaningful when `enable_timeout` bit is set to true in
  // `GMBusCommand` (GMBUS1) register.
  //
  // The Programmer's Reference Manual doesn't mention how to reset the bit, but
  // it's possible that a controller local reset can reset it.
  DEF_BIT(13, target_stall_timeout_occurred);

  DEF_RSVDZ_BIT(12);

  // If true, GMBUS hardware enters a "ready" state and will not perform any
  // operation until the software acts.
  //
  // Certain actions should not be performed while this bit is false. For
  // example, the semantics of reading/writing the `GMBusData` register are more
  // complex when this bit is false.
  //
  // This bit transitions from false to true when:
  // - GMBUS is waiting for the software to read/write the `GMBusData` register
  //   in order to advance an I2C read/write transaction.
  // - A GMBUS cycle ended with a STOP.
  // - GMBUS completes the partial reset procedure initiated via the
  //   `GMBusCommand` register.
  //
  // This bit transitions from true to false when:
  // - A new I2C transaction over GMBUS is triggered.
  // - GMBUS partial reset is initiated via the `GMBusCommand` register.
  //
  // This bit is also known as "hardware ready" (or HW_RDY) bit in Intel
  // Programmer's Reference Manual.
  DEF_BIT(11, is_ready);

  // True if the receiver device doesn't send an acknowledgement signal after a
  // data byte is transmitted, which means a "not acknowledge (NACK)" in I2C.
  //
  // This bit can only be cleared by performing a reset via the `GMBusCommand`
  // (GMBUS1) register.
  DEF_BIT(10, nack_occurred);

  // True if the controller is in an active state, false if the controller is
  // in an idle state.
  //
  // Active states are: START (including RESTART), ADDRESS, INDEX, DATA, WAIT
  // and STOP phase.
  DEF_BIT(9, is_active);

  // Can be used to determine the number of bytes currently transmitted /
  // received by the GMBUS controller hardware. Controller sets it to zero at
  // the start of a GMBUS transaction data transfer and increments it after the
  // completion of each byte of the data phase.
  //
  // Note that because reads have internal storage, the byte count on a read
  // operation may be ahead of the data that has been accepted from the data
  // register.
  DEF_FIELD(8, 0, current_byte_count);

  static auto Get() { return hwreg::RegisterAddr<GMBusControllerStatus>(0xc5108); }
};

// GMBUS3
// (Graphic Management Bus Configuration Register 3 -- Data)
//
// This register stores data bits staged for the GMBUS controller to write, or
// data bits retrieved by the GMBUS controller.
//
// This register must be accessed only when `is_ready` bit of
// `GMBusControllerStatus` register is true. Otherwise the behavior is
// undefined. The value of this register is double-buffered on the `is_ready`
// bit.
//
// - During write cycles, the data bits staged to this register will be copied
//   into an internal I2C write buffer and written onto the I2C bus by GMBUS
//   controller, during which `is_ready` bit will become false. Writing data to
//   this register triggers this procedure.
//
// - During read cycles, the data bits read from the I2C bus by GMBUS controller
//   are copied from the internal I2C write buffer to this register. Reading
//   this register causes `is_ready` bit to become false and triggers a new read
//   cycle.
//
// - The least significant bit (bit 0) is first to read / write in I2C
//   transaction and the most significant bit (bit 31) is the last bit to read /
//   write.
//
// This register doesn't contain any reserved fields / bits. So, the register
// can be safely updated without reading it first.
//
// This register is written protected when `software_clear_interrupt` in
// `GMBusCommand` (GMBUS1) register is enabled.
//
// Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev 2.0 Part 1, Page 1028
// Kaby Lake: IHD-OS-KBL-Vol 2c-1.17 Part 1, Page 736
// Skylake: IHD-OS-SKL-Vol 2c-05.16 Part 1, Page 730
class GMBusData : public hwreg::RegisterBase<GMBusData, uint32_t> {
 public:
  // Data byte #3 (most significant byte).
  // The helpers `data()` and `set_data()` should be preferred to accessing the
  // field directly.
  DEF_FIELD(31, 24, data_byte_3);

  // Data byte #2.
  // The helpers `data()` and `set_data()` should be preferred to accessing the
  // field directly.
  DEF_FIELD(23, 16, data_byte_2);

  // Data byte #1.
  // The helpers `data()` and `set_data()` should be preferred to accessing the
  // field directly.
  DEF_FIELD(15, 8, data_byte_1);

  // Data byte #0 (least significant byte).
  // The helpers `data()` and `set_data()` should be preferred to accessing the
  // field directly.
  DEF_FIELD(7, 0, data_byte_0);

  // Helper method getting `data_byte_3`, `data_byte_2`, `data_byte_1` and
  // `data_byte_0` fields read from the data register.
  //
  // This always returns all four bytes. The caller must only use the bytes
  // within `total_byte_count` range.
  std::array<const uint8_t, 4> data() const {
    return {
        static_cast<uint8_t>(data_byte_0()),
        static_cast<uint8_t>(data_byte_1()),
        static_cast<uint8_t>(data_byte_2()),
        static_cast<uint8_t>(data_byte_3()),
    };
  }

  // Helper method setting `data_byte_3`, `data_byte_2`, `data_byte_1` and
  // `data_byte_0` fields to write to the data register.
  //
  // `data` must have at most 4 elements.
  GMBusData& set_data(std::span<const uint8_t> data) {
    ZX_DEBUG_ASSERT(data.size() <= 4);
    if (!data.empty()) {
      set_data_byte_0(data[0]);
    }
    if (data.size() > 1) {
      set_data_byte_1(data[1]);
    }
    if (data.size() > 2) {
      set_data_byte_2(data[2]);
    }
    if (data.size() > 3) {
      set_data_byte_3(data[3]);
    }
    return *this;
  }

  static auto Get() { return hwreg::RegisterAddr<GMBusData>(0xc510c); }
};

// GMBUS4
// (Graphic Management Bus Configuration Register 4 -- Interrupt Mask)
//
// This register specifies the GMBUS events that can trigger a display
// engine interrupt.
//
// All reserved bits in this register are MBZ (must be zero). So, the register
// can be safely updated without reading it first.
//
// This register is written protected when `software_clear_interrupt` in
// `GMBusCommand` (GMBUS1) register is enabled.
//
// Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev 2.0 Part 1, Page 1029
// Kaby Lake: IHD-OS-KBL-Vol 2c-1.17 Part 1, Page 737
// Skylake: IHD-OS-SKL-Vol 2c-05.16 Part 1, Page 731
class GMBusControllerInterruptMask
    : public hwreg::RegisterBase<GMBusControllerInterruptMask, uint32_t> {
 public:
  DEF_RSVDZ_FIELD(31, 5);

  // If this bit is true, when `target_stall_timeout` bit in
  // `GMBusControllerStatus` (GMBUS2) register is asserted, the south display
  // engine will trigger an interrupt, and the `gmbus` bit in `SdeInterruptBase`
  // status register will be set to true.
  DEF_BIT(4, target_stall_timeout_interrupt_enabled);

  // If this bit is true, when `nack_occurred` bit in `GMBusControllerStatus`
  // (GMBUS2) register is asserted, the south display engine will trigger an
  // interrupt, and the `gmbus` bit in `SdeInterruptBase` status register will
  // be set to true.
  DEF_BIT(3, nack_occurred_interrupt_enabled);

  // If this bit is true, when `is_active` bit in `GMBusControllerStatus`
  // (GMBUS2) register is de-asserted, the south display engine will trigger an
  // interrupt, and the `gmbus` bit in `SdeInterruptBase` status register will
  // be set to true.
  DEF_BIT(2, is_idle_interrupt_enabled);

  // If this bit is true, when `is_waiting` bit in `GMBusControllerStatus`
  // (GMBUS2) register is asserted, the south display engine will trigger an
  // interrupt, and the `gmbus` bit in `SdeInterruptBase` status register will
  // be set to true.
  DEF_BIT(1, is_waiting_interrupt_enabled);

  // If this bit is true, when `is_ready` bit in `GMBusControllerStatus`
  // (GMBUS2) register is asserted, the south display engine will trigger an
  // interrupt, and the `gmbus` bit in `SdeInterruptBase` status register will
  // be set to true.
  DEF_BIT(0, is_ready_interrupt_enabled);

  static auto Get() { return hwreg::RegisterAddr<GMBusControllerInterruptMask>(0xc5110); }
};

// GMBUS5
// (Graphic Management Bus Configuration Register 5 -- 2 Byte Index)
//
// This register specifies the 2-byte device index if the driver opts to write
// 2 bytes at the INDEX phase of a cycle.
//
// All reserved bits in this register are MBZ (must be zero). So, the register
// can be safely updated without reading it first.
//
// Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev 2.0 Part 1, Page 1030
// Kaby Lake: IHD-OS-KBL-Vol 2c-1.17 Part 1, Page 738
// Skylake: IHD-OS-SKL-Vol 2c-05.16 Part 1, Page 732
class GMBusTwoByteIndex : public hwreg::RegisterBase<GMBusTwoByteIndex, uint32_t> {
 public:
  // This bit enables 2-byte INDEX phase so that the controller will write
  // 2 bytes (`two_byte_target_index`) during the INDEX phase
  // instead of `target_index` in `GMBusCommand` (GMBUS1) register.
  DEF_BIT(31, two_byte_index_enable);

  DEF_RSVDZ_FIELD(30, 16);

  // This is the 2-byte index used in all GMBUS accesses when
  // `two_byte_index_enable` is true.
  //
  // The least significant bit (bit 0) will be transferred the first, and the
  // most significant bit (bit 15) will be transferred the last.
  DEF_FIELD(15, 0, two_byte_target_index);

  static auto Get() { return hwreg::RegisterAddr<GMBusTwoByteIndex>(0xc5120); }
};

// GPIO_CTL
// (GPIO (general-purpose input/output) Pin Pair Control)
//
// Intel Display Engine has a set of GPIO pin pairs each of which can be
// controlled independently for a certain DDI, allowing the support of device
// query (EDID) and control functions (DDC interface protocols).
//
// The two pins in each pin pair are named "data" and "clock" based on their
// typical roles ("data" pin for data line (SDA), and "clock" pin for clock line
// (SDC)) in I2C protocol, though they can be used on other purposes. For
// example, Tiger Lake may use the same GPIO pins for MIPI DSI power and control
// (IHD-OS-TGL-Vol 12-1.22-Rev 2.0, Page 119).
//
// These GPIO pins can be programmed by GMBUS controller, or the
// software can manual program the pins directly (also known as "bit banging"
// or "bit bashing"). Details about bit bashing is available at Programmers'
// Reference Manual, Section "GPIO Programming for I2C Bit Bashing":
// - Tiger Lake: IHD-OS-TGL-Vol 12-1.22-Rev 2.0, Page 424
// - Kaby Lake: IHD-OS-KBL-Vol 12-1.17, Page 199
// - Skylake: IHD-OS-SKL-Vol 12-05.16, Page 191
//
// This register has an unusual scheme to support partial modification without
// full read-modify-write operation. Each of the 1-bit fields
// {data, clock}_{value, direction_is_output} is backed by a write_* bit.
// When a MMIO write has a write_* bit set to zero, the corresponding field is
// ignored during the write and the old value of that field will be preserved.
//
// - Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev 2.0, Part 1, pages 1030-1034
// - Kaby Lake: IHD-OS-KBL-Vol 2c-1.17, Part 1, pages 756-758
// - Skylake: IHD-OS-SKL-Vol 2c-05.16, Part 1, pages 750-752
class GpioPinPairControl : public hwreg::RegisterBase<GpioPinPairControl, uint32_t> {
 public:
  DEF_RSVDZ_FIELD(31, 13);

  // The signal sampled on GPIO data pin as input.
  DEF_BIT(12, data_input);

  // The value that will be placed on GPIO data pin as output.
  // The pin will be driven to the value here iff `data_direction_is_output` is
  // true.
  //
  // In order to update this bit when writing to the register,
  // `write_data_output` must be true. See the register-level description about
  // `write_*` fields.
  DEF_BIT(11, data_output);

  // If this field is true when writing to the register, the
  // `data_output` field of the register will be overwritten by the new value;
  // otherwise the old value will be preserved.
  // See the register-level description about `write_*` fields.
  DEF_BIT(10, write_data_output);

  // Whether the GPIO pin will be driven to the value of the `data_output`
  // field.
  //
  // In order to update this bit when writing to the register,
  // `write_data_direction_is_output` must be true.\ See the register-level
  // description about `write_*` fields.
  DEF_BIT(9, data_direction_is_output);

  // If this field is true when writing to the register, the
  // `data_direction_is_output` field of the register will be overwritten by the
  // new value; otherwise the old value will be preserved. See the
  // register-level description about `write_*` fields.
  DEF_BIT(8, write_data_direction_is_output);

  DEF_RSVDZ_FIELD(7, 5);

  // Equivalent of `data_input` for the clock pin.
  DEF_BIT(4, clock_input);

  // Equivalent of `data_output` for the clock pin.
  DEF_BIT(3, clock_output);

  // Equivalent of `write_data_output` for the clock pin.
  DEF_BIT(2, write_clock_output);

  // Equivalent of `data_direction_is_output` for the clock pin.
  DEF_BIT(1, clock_direction_is_output);

  // Equivalent of `write_data_direction_is_output` for the clock pin.
  DEF_BIT(0, write_clock_direction_is_output);

  // Get the DDC GPIO pin control register for port `gpio_port`.
  static auto GetForPort(const intel_display::GpioPort& gpio_port) {
    int gpio_port_id = gpio_port.number();
    return hwreg::RegisterAddr<GpioPinPairControl>(0xc5010 + 4 * gpio_port_id);
  }
};

}  // namespace registers

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_REGISTERS_GMBUS_H_
