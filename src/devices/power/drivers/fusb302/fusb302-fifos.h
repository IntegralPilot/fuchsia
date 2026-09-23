// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_POWER_DRIVERS_FUSB302_FUSB302_FIFOS_H_
#define SRC_DEVICES_POWER_DRIVERS_FUSB302_FUSB302_FIFOS_H_

// The comments in this file reference the USB Power Delivery Specification,
// downloadable at https://usb.org/document-library/usb-power-delivery
//
// usbpd3.1 is Revision 3.1, Version 1.7, published January 2023.

#include <fidl/fuchsia.hardware.i2c/cpp/wire.h>
#include <lib/stdcompat/span.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>

#include <cstdint>
#include <optional>

#include "src/devices/power/drivers/fusb302/usb-pd-message.h"

namespace fusb302 {

// Manages the Tx (transmitter) and Rx (receiver) FIFOS on the chip.
//
// This class moves PD message data between `usb_pd::Message` instances and the
// BMC (Bi-phase Mark Encoding) PHY's internal buffers. This involves format
// conversion, and communication over the I2C-based FIFO interface.
class Fusb302Fifos {
 public:
  // `i2c_channel` must remain alive throughout the new instance's lifetime.
  explicit Fusb302Fifos(fidl::ClientEnd<fuchsia_hardware_i2c::Device>& i2c_channel);

  Fusb302Fifos(const Fusb302Fifos&) = delete;
  Fusb302Fifos& operator=(const Fusb302Fifos&) = delete;

  // Trivially destructible.
  ~Fusb302Fifos() = default;

  // Submits a PD message to the PHY layer's transmit FIFO.
  //
  // Returns an error if an I/O error occurs while communicating with the
  // FUSB302 over I2C.
  //
  // Conceptually, this method handles the PHY layer concerns in the USB PD
  // spec.  Callers are responsible for the PD protocol concerns. See usbpd3.1
  // 6.12.2.2 "Protocol Layer Message Transmission" and usbpd3.1 6.12.2.3
  // "Protocol Layer Message Reception".
  //
  // However, this method performs the minimum amount of flow control to ensure
  // that the PHY does not drop the message. This comes down to busy-looping
  // while the CC wire is in use (the FUSB302 drops messages when it detects
  // collisions), and busy-looping while another transmission is in progress
  // (the FUSB302 is flaky if we attempt to fill the FIFO with more than one
  // message).
  zx::result<> TransmitMessage(const usb_pd::Message& message);

  // Reads a message from the PHY layer's receive FIFO.
  //
  // Returns an error if an I/O error occurs while communicating with the
  // FUSB302 over I2C. Returns a null option if the receive FIFO is empty.
  //
  // Conceptually, this method handles the PHY layer concerns in the USB PD
  // spec. Callers are responsible for the PD protocol concerns.
  zx::result<std::optional<usb_pd::Message>> ReadReceivedMessage();

 private:
  zx::result<> FifoI2cRead(cpp20::span<uint8_t> read_output);

  // Writes multiple bytes to the FIFO register.
  //
  // `i2c_write_bytes` must start with the address of the Fifos register.
  //
  // The API is not symmetric with `FifoI2cRead()` in the interest of
  // efficiency. Specifically, taking in a buffer of FIFO write data would
  // require submitting two I2C transacstion objects, or a buffer copy. This
  // wrinkle (API asymmetry) is acceptable in a class implementation detail.
  zx::result<> FifoI2cWrite(cpp20::span<uint8_t> i2c_write_bytes);

  // Guaranteed to outlive this instance, because it's owned by this instance's
  // owner (Fusb302).
  fidl::ClientEnd<fuchsia_hardware_i2c::Device>& i2c_;
};

}  // namespace fusb302

#endif  // SRC_DEVICES_POWER_DRIVERS_FUSB302_FUSB302_FIFOS_H_
