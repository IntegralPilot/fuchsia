// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_DESIGNWARE_HDMI_DDC_CONTROLLER_REGS_H_
#define SRC_GRAPHICS_DISPLAY_LIB_DESIGNWARE_HDMI_DDC_CONTROLLER_REGS_H_

#include <hwreg/bitfields.h>

namespace designware_hdmi::registers {

// i2cm_slave (I2C DDC Target address Configuration Register)
//
// HDMI Transmitter Controller Databook, Section 6.17.1, page 490
class DdcControllerDataTargetAddress
    : public hwreg::RegisterBase<DdcControllerDataTargetAddress, uint8_t> {
 public:
  static hwreg::RegisterAddr<DdcControllerDataTargetAddress> Get() {
    return hwreg::RegisterAddr<DdcControllerDataTargetAddress>(0x7e00);
  }

  // I2C target address of the DDC / E-DDC data.
  DEF_FIELD(6, 0, data_target_address);
};

// i2cm_address (I2C DDC Address Configuration Register)
//
// HDMI Transmitter Controller Databook, Section 6.17.2, page 491
class DdcControllerWordOffset : public hwreg::RegisterBase<DdcControllerWordOffset, uint8_t> {
 public:
  static hwreg::RegisterAddr<DdcControllerWordOffset> Get() {
    return hwreg::RegisterAddr<DdcControllerWordOffset>(0x7e01);
  }

  // Word offset within the segment, as specified in E-DDC Standard, Section 6
  // "Command Structures".
  DEF_FIELD(7, 0, word_offset);
};

// i2cm_datao (I2C DDC Data Write Register)
//
// HDMI Transmitter Controller Databook, Section 6.17.3, page 491
class DdcControllerWriteByte : public hwreg::RegisterBase<DdcControllerWriteByte, uint8_t> {
 public:
  static hwreg::RegisterAddr<DdcControllerWriteByte> Get() {
    return hwreg::RegisterAddr<DdcControllerWriteByte>(0x7e02);
  }

  // The byte to be written on the display device at the address specified by
  // `DdcControllerWordOffset`.
  //
  // Used when a `write` command is requested in `DdcControllerCommand`.
  DEF_FIELD(7, 0, byte);
};

// i2cm_datai (I2C DDC Data read Register)
//
// HDMI Transmitter Controller Databook, Section 6.17.4, page 492
class DdcControllerReadByte : public hwreg::RegisterBase<DdcControllerReadByte, uint8_t> {
 public:
  static hwreg::RegisterAddr<DdcControllerReadByte> Get() {
    return hwreg::RegisterAddr<DdcControllerReadByte>(0x7e03);
  }

  // The byte read from the display device at the address specified by
  // `DdcControllerWordOffset`.
  //
  // Used when `ddc_read_byte` or `eddc_read_byte` command is requested
  // in `DdcControllerCommand`.
  DEF_FIELD(7, 0, byte);
};

// i2cm_operation (I2C DDC RD / RD_EXT / WR Operation Register)
//
// This register is write-only. Reading this register always results in 0x00.
//
// HDMI Transmitter Controller Databook, Section 6.17.5, page 493
class DdcControllerCommand : public hwreg::RegisterBase<DdcControllerCommand, uint8_t> {
 public:
  static hwreg::RegisterAddr<DdcControllerCommand> Get() {
    return hwreg::RegisterAddr<DdcControllerCommand>(0x7e04);
  }

  // Exactly one of the following bits must be true. The remaining bits must
  // be false.

  // Clears the I2C bus.
  DEF_BIT(5, clear_bus);

  // Writes one byte to the display device.
  DEF_BIT(4, write);

  // Reads 8 bytes in extended (E-DDC) mode, where the segment pointer is sent
  // to the display device.
  DEF_BIT(3, eddc_read_8bytes);

  // Reads 8 bytes in normal (DDC) mode, where the segment pointer is not sent
  // to the display device.
  DEF_BIT(2, ddc_read_8bytes);

  // Reads one byte in extended (E-DDC) mode, where the segment pointer is sent
  // to the display device.
  DEF_BIT(1, eddc_read_byte);

  // Reads one byte in normal (DDC) mode, where the segment pointer is not sent
  // to the display device.
  DEF_BIT(0, ddc_read_byte);
};

// ih_i2cm_stat0 (E-DDC I2C Controller Interrupt Status Register)
//
// HDMI Transmitter Controller Databook, Section 6.2.6, page 202
class DdcControllerInterruptStatus
    : public hwreg::RegisterBase<DdcControllerInterruptStatus, uint8_t> {
 public:
  static hwreg::RegisterAddr<DdcControllerInterruptStatus> Get() {
    return hwreg::RegisterAddr<DdcControllerInterruptStatus>(0x105);
  }

  // True iff there is an active read request.
  // Writing true clears the interrupt signal.
  DEF_BIT(2, read_request_pending);

  // True iff the previous controller command is done.
  // Writing true clears the interrupt signal.
  DEF_BIT(1, command_done_pending);

  // True iff the previous controller command failed with an error.
  // Writing true clears the interrupt signal.
  DEF_BIT(0, error_pending);
};

// ih_mute_i2cm_stat0 (E-DDC I2C Controller Interrupt Mute Control Register)
//
// The bits in this register mute corresponding I2C controller interrupts.
// Muting is different from masking. A masked interrupt will neither trigger
// the I2C controller interrupt signal nor assert the interrupt status register
// bit. A muted interrupt doesn't trigger the interrupt but asserts the
// interrupt status register bit.
//
// HDMI Transmitter Controller Databook, Section 6.2.17, page 213
class DdcControllerInterruptMute : public hwreg::RegisterBase<DdcControllerInterruptMute, uint8_t> {
 public:
  static hwreg::RegisterAddr<DdcControllerInterruptMute> Get() {
    return hwreg::RegisterAddr<DdcControllerInterruptMute>(0x185);
  }

  // True iff the read request interrupt is muted.
  DEF_BIT(2, read_request_muted);

  // True iff the controller command done interrupt is muted.
  DEF_BIT(1, command_done_muted);

  // True iff the controller error interrupt is muted.
  DEF_BIT(0, error_muted);
};

// i2cm_int (I2C DDC Done Interrupt Register)
//
// The bits in this register mask corresponding I2C controller interrupts.
// A masked interrupt will neither trigger the I2C controller interrupt signal
// nor assert the interrupt status register bit.
//
// HDMI Transmitter Controller Databook, Section 6.17.6, page 494
class DdcControllerDoneInterruptMask
    : public hwreg::RegisterBase<DdcControllerDoneInterruptMask, uint8_t> {
 public:
  static hwreg::RegisterAddr<DdcControllerDoneInterruptMask> Get() {
    return hwreg::RegisterAddr<DdcControllerDoneInterruptMask>(0x7e05);
  }

  // True iff the read request interrupt is masked.
  DEF_BIT(6, read_request_masked);

  // True iff the command done interrupt is masked.
  DEF_BIT(2, command_done_masked);
};

// i2cm_ctlint (I2C DDC Error Interrupt Register)
//
// The bits in this register mask corresponding I2C controller error interrupts.
// A masked interrupt will neither trigger the I2C controller error interrupt
// signal nor assert the interrupt status register error bit.
//
// HDMI Transmitter Controller Databook, Section 6.17.7, page 495
class DdcControllerErrorInterruptMask
    : public hwreg::RegisterBase<DdcControllerErrorInterruptMask, uint8_t> {
 public:
  static hwreg::RegisterAddr<DdcControllerErrorInterruptMask> Get() {
    return hwreg::RegisterAddr<DdcControllerErrorInterruptMask>(0x7e06);
  }

  // True iff the Not Acknowledged (NACK) error interrupt is masked.
  DEF_BIT(6, nack_masked);

  // True iff the arbitration error interrupt is masked.
  DEF_BIT(2, arbitration_masked);
};

// i2cm_div (I2C DDC Speed Control Register)
//
// HDMI Transmitter Controller Databook, Section 6.17.8, page 496
class DdcControllerClockControl : public hwreg::RegisterBase<DdcControllerClockControl, uint8_t> {
 public:
  enum class I2cControllerTransferMode : uint8_t {
    // Data can be transferred at up to 400 kbit/s.
    kFastMode = 1,

    // Data can be transferred at up to 100 kbit/s.
    kStandardMode = 0,
  };

  static hwreg::RegisterAddr<DdcControllerClockControl> Get() {
    return hwreg::RegisterAddr<DdcControllerClockControl>(0x7e07);
  }

  DEF_ENUM_FIELD(I2cControllerTransferMode, 3, 3, i2c_controller_transfer_mode);
};

// i2cm_segaddr (I2C DDC Segment Address Configuration Register)
//
// HDMI Transmitter Controller Databook, Section 6.17.9, page 496
class DdcControllerSegmentTargetAddress
    : public hwreg::RegisterBase<DdcControllerSegmentTargetAddress, uint8_t> {
 public:
  static hwreg::RegisterAddr<DdcControllerSegmentTargetAddress> Get() {
    return hwreg::RegisterAddr<DdcControllerSegmentTargetAddress>(0x7e08);
  }

  // I2C target address of the segment pointer.
  //
  // Used when `eddc_read_byte` or `eddc_read_8bytes` command is requested
  // in `DdcControllerCommand`.
  DEF_FIELD(6, 0, segment_target_address);
};

// i2cm_softrstz (I2C DDC Soft Reset Control Register)
//
// HDMI Transmitter Controller Databook, Section 6.17.10, page 497
class DdcControllerSoftReset : public hwreg::RegisterBase<DdcControllerSoftReset, uint8_t> {
 public:
  static hwreg::RegisterAddr<DdcControllerSoftReset> Get() {
    return hwreg::RegisterAddr<DdcControllerSoftReset>(0x7e09);
  }

  // When a zero is written to this bit, the I2C controller is reset.
  DEF_BIT(0, reset);
};

// i2cm_segptr (I2C DDC Segment Pointer Register)
//
// HDMI Transmitter Controller Databook, Section 6.17.11, page 497
class DdcControllerSegmentPointer
    : public hwreg::RegisterBase<DdcControllerSegmentPointer, uint8_t> {
 public:
  static hwreg::RegisterAddr<DdcControllerSegmentPointer> Get() {
    return hwreg::RegisterAddr<DdcControllerSegmentPointer>(0x7e0a);
  }

  // Value of the segment pointer.
  //
  // Used when `eddc_read_byte` or `eddc_read_8bytes` command is requested
  // in `DdcControllerCommand`.
  DEF_FIELD(7, 0, segment_pointer);
};

// i2cm_ss_scl_hcnt_1_addr (I2C DDC Slow Speed SCL High Level Control Register 1)
//
// HDMI Transmitter Controller Databook, Section 6.17.12, page 498
class DdcControllerSlowSpeedSclHighLevelControl1
    : public hwreg::RegisterBase<DdcControllerSlowSpeedSclHighLevelControl1, uint8_t> {
 public:
  static hwreg::RegisterAddr<DdcControllerSlowSpeedSclHighLevelControl1> Get() {
    return hwreg::RegisterAddr<DdcControllerSlowSpeedSclHighLevelControl1>(0x7e0b);
  }
};

// i2cm_ss_scl_hcnt_0_addr (I2C DDC Slow Speed SCL High Level Control Register 0)
//
// HDMI Transmitter Controller Databook, Section 6.17.13, page 498
class DdcControllerSlowSpeedSclHighLevelControl0
    : public hwreg::RegisterBase<DdcControllerSlowSpeedSclHighLevelControl0, uint8_t> {
 public:
  static hwreg::RegisterAddr<DdcControllerSlowSpeedSclHighLevelControl0> Get() {
    return hwreg::RegisterAddr<DdcControllerSlowSpeedSclHighLevelControl0>(0x7e0c);
  }
};

// i2cm_ss_scl_lcnt_1_addr (I2C DDC Slow Speed SCL Low Level Control Register 1)
//
// HDMI Transmitter Controller Databook, Section 6.17.14, page 499
class DdcControllerSlowSpeedSclLowLevelControl1
    : public hwreg::RegisterBase<DdcControllerSlowSpeedSclLowLevelControl1, uint8_t> {
 public:
  static hwreg::RegisterAddr<DdcControllerSlowSpeedSclLowLevelControl1> Get() {
    return hwreg::RegisterAddr<DdcControllerSlowSpeedSclLowLevelControl1>(0x7e0d);
  }
};

// i2cm_ss_scl_lcnt_0_addr (I2C DDC Slow Speed SCL Low Level Control Register 0)
//
// HDMI Transmitter Controller Databook, Section 6.17.15, page 499
class DdcControllerSlowSpeedSclLowLevelControl0
    : public hwreg::RegisterBase<DdcControllerSlowSpeedSclLowLevelControl0, uint8_t> {
 public:
  static hwreg::RegisterAddr<DdcControllerSlowSpeedSclLowLevelControl0> Get() {
    return hwreg::RegisterAddr<DdcControllerSlowSpeedSclLowLevelControl0>(0x7e0e);
  }
};

// i2cm_fs_scl_hcnt_1_addr (I2C DDC Fast Speed SCL High Level Control Register 1)
//
// HDMI Transmitter Controller Databook, Section 6.17.16, page 500
class DdcControllerFastSpeedSclHighLevelControl1
    : public hwreg::RegisterBase<DdcControllerFastSpeedSclHighLevelControl1, uint8_t> {
 public:
  static hwreg::RegisterAddr<DdcControllerFastSpeedSclHighLevelControl1> Get() {
    return hwreg::RegisterAddr<DdcControllerFastSpeedSclHighLevelControl1>(0x7e0f);
  }
};

// i2cm_fs_scl_hcnt_0_addr (I2C DDC Fast Speed SCL High Level Control Register 0)
//
// HDMI Transmitter Controller Databook, Section 6.17.17, page 500
class DdcControllerFastSpeedSclHighLevelControl0
    : public hwreg::RegisterBase<DdcControllerFastSpeedSclHighLevelControl0, uint8_t> {
 public:
  static hwreg::RegisterAddr<DdcControllerFastSpeedSclHighLevelControl0> Get() {
    return hwreg::RegisterAddr<DdcControllerFastSpeedSclHighLevelControl0>(0x7e10);
  }
};

// i2cm_fs_scl_lcnt_1_addr (I2C DDC Fast Speed SCL Low Level Control Register 1)
//
// HDMI Transmitter Controller Databook, Section 6.17.18, page 501
class DdcControllerFastSpeedSclLowLevelControl1
    : public hwreg::RegisterBase<DdcControllerFastSpeedSclLowLevelControl1, uint8_t> {
 public:
  static hwreg::RegisterAddr<DdcControllerFastSpeedSclLowLevelControl1> Get() {
    return hwreg::RegisterAddr<DdcControllerFastSpeedSclLowLevelControl1>(0x7e11);
  }
};

// i2cm_fs_scl_lcnt_0_addr (I2C DDC Fast Speed SCL Low Level Control Register 0)
//
// HDMI Transmitter Controller Databook, Section 6.17.19, page 501
class DdcControllerFastSpeedSclLowLevelControl0
    : public hwreg::RegisterBase<DdcControllerFastSpeedSclLowLevelControl0, uint8_t> {
 public:
  static hwreg::RegisterAddr<DdcControllerFastSpeedSclLowLevelControl0> Get() {
    return hwreg::RegisterAddr<DdcControllerFastSpeedSclLowLevelControl0>(0x7e12);
  }
};

// i2cm_sda_hold (I2C DDC SDA Hold Time Configuration Register)
//
// HDMI Transmitter Controller Databook, Section 6.17.20, page 502
class DdcControllerDataPinHoldTime
    : public hwreg::RegisterBase<DdcControllerDataPinHoldTime, uint8_t> {
 public:
  static hwreg::RegisterAddr<DdcControllerDataPinHoldTime> Get() {
    return hwreg::RegisterAddr<DdcControllerDataPinHoldTime>(0x7e13);
  }

  // Hold time of the I2C SDA (data) pin, measured in SFR (Special Function
  // Register) clock cycles.
  //
  // In a valid configuration, the hold time must be >= 300 ns.
  // data_pin_hold_time = ceil(300 ns / (1 / SFR clock frequency))
  DEF_FIELD(7, 0, data_pin_hold_time);
};

// i2cm_scdc_read_update (SCDC Control Register)
//
// HDMI Transmitter Controller Databook, Section 6.17.21, page 503
class DdcControllerScdcControl : public hwreg::RegisterBase<DdcControllerScdcControl, uint8_t> {
 public:
  static hwreg::RegisterAddr<DdcControllerScdcControl> Get() {
    return hwreg::RegisterAddr<DdcControllerScdcControl>(0x7e14);
  }

  // Experiments on VIM3 (Amlogic A311D) show that setting this register to
  // 0x00 disables all SCDC (Status and Control Data Channel) operations.
  static constexpr uint8_t kDisableAllScdcOperations = 0x00;
};

// i2cm_read_buff0 (I2C DDC Read Buffer 0 Register)
//
// This register also represents i2cm_read_buff1-7 (I2C DDC Read Buffer 1-7
// Register).
//
// HDMI Transmitter Controller Databook, Section 6.17.22-6.17.29, pages 504-506
class DdcControllerReadBuffer : public hwreg::RegisterBase<DdcControllerReadBuffer, uint8_t> {
 public:
  static hwreg::RegisterAddr<DdcControllerReadBuffer> Get(int offset) {
    // Address of i2cm_read_buff0: 0x7e20
    // Address of i2cm_read_buff1: 0x7e21
    // ...
    // Address of i2cm_read_buff7: 0x7e27
    ZX_DEBUG_ASSERT(offset >= 0);
    ZX_DEBUG_ASSERT(offset <= 7);
    return hwreg::RegisterAddr<DdcControllerReadBuffer>(0x7e20 + offset);
  }

  // The byte read from the display device.
  //
  // Only holds a meaningful value after a `ddc_read_8bytes` or
  // `eddc_read_8bytes` command is requested in `DdcControllerCommand`.
  DEF_FIELD(7, 0, byte);
};

}  // namespace designware_hdmi::registers

#endif  // SRC_GRAPHICS_DISPLAY_LIB_DESIGNWARE_HDMI_DDC_CONTROLLER_REGS_H_
