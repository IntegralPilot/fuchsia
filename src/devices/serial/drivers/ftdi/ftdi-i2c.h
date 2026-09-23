// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_SERIAL_DRIVERS_FTDI_FTDI_I2C_H_
#define SRC_DEVICES_SERIAL_DRIVERS_FTDI_FTDI_I2C_H_

#include <fidl/fuchsia.hardware.ftdi/cpp/wire.h>
#include <fidl/fuchsia.hardware.i2c.businfo/cpp/fidl.h>
#include <fidl/fuchsia.hardware.i2cimpl/cpp/driver/wire.h>
#include <lib/driver/outgoing/cpp/outgoing_directory.h>
#include <threads.h>

#include <vector>

#include <ddktl/device.h>
#include <ddktl/metadata_server.h>

#include "ftdi-mpsse.h"
#include "ftdi.h"

namespace ftdi_mpsse {

class FtdiI2c;
using DeviceType = ddk::Device<FtdiI2c, ddk::Initializable, ddk::Unbindable>;

// This class represents a single I2C bus created from 3 pins of an FTDI device.
// It implements the standard I2cImpl driver. It is created with metadata that will
// allow other I2C devices that exist on the bus to bind.
class FtdiI2c : public DeviceType, public fdf::WireServer<fuchsia_hardware_i2cimpl::Device> {
 public:
  struct I2cLayout {
    uint32_t scl;
    uint32_t sda_out;
    uint32_t sda_in;
  };
  struct I2cDevice {
    uint32_t address;
    uint32_t vid;
    uint32_t pid;
    uint32_t did;
  };
  FtdiI2c(zx_device_t* parent, ftdi_serial::FtdiSerial* serial, I2cLayout layout,
          std::vector<I2cDevice> i2c_devices)
      : DeviceType(parent),
        pin_layout_(layout),
        mpsse_(serial),
        i2c_devices_(std::move(i2c_devices)),
        outgoing_(fdf::Dispatcher::GetCurrent()->get()) {}

  static zx_status_t Create(zx_device_t* device, ftdi_serial::FtdiSerial* serial,
                            const fuchsia_hardware_ftdi::wire::I2cBusLayout* layout,
                            const fuchsia_hardware_ftdi::wire::I2cDevice* i2c_dev);

  zx_status_t Bind();
  void DdkInit(ddk::InitTxn txn);
  void DdkUnbind(ddk::UnbindTxn txn);
  void DdkRelease() { delete this; }

  void GetMaxTransferSize(fdf::Arena& arena,
                          GetMaxTransferSizeCompleter::Sync& completer) override {
    completer.buffer(arena).ReplySuccess(kFtdiI2cMaxTransferSize);
  }

  // Sets the bitrate for the i2c bus in KHz units.
  void SetBitrate(SetBitrateRequestView request, fdf::Arena& arena,
                  SetBitrateCompleter::Sync& completer) override {
    completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  void Transact(TransactRequestView request, fdf::Arena& arena,
                TransactCompleter::Sync& completer) override;

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_i2cimpl::Device> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override;

  zx_status_t Ping(uint8_t bus_address);
  zx_status_t Transact(uint8_t bus_address, std::vector<uint8_t> write_data,
                       std::vector<uint8_t>* read_data);
  zx_status_t Enable();

 private:
  static constexpr int kFtdiI2cMaxTransferSize = 0x1000;
  static constexpr uint8_t kI2cWriteCommandByte1 = 0x11;
  static constexpr uint8_t kI2cWriteCommandByte2 = 0x00;
  static constexpr uint8_t kI2cWriteCommandByte3 = 0x00;
  static constexpr uint8_t kI2cReadAckCommandByte1 = 0x22;
  static constexpr uint8_t kI2cReadAckCommandByte2 = 0x00;
  static constexpr uint8_t kI2cReadOneByteCommand[] = {0x20, 0x00, 0x00, 0x13, 0x00, 0x00};
  static constexpr uint8_t kI2cReadFinalByteCommand[] = {0x20, 0x00, 0x00, 0x13, 0x00, 0xFF};
  // Every full write requires 49 additional bytes. These are for the start and end I2C
  // sequence commands.
  static constexpr uint8_t kI2cNumCommandBytesPerFullWrite = 49;
  static constexpr uint8_t kI2cNumCommandBytesPerFullReadWrite =
      kI2cNumCommandBytesPerFullWrite + 48;
  // We need to write 12 bytes for every written byte. There are 3 prefix command bytes, a 6
  // byte command to reset GPIO pins, and a 2 byte suffix command for reading the ACK bit.
  static constexpr uint8_t kI2cNumCommandBytesPerWriteByte = 12;
  static constexpr uint8_t kI2cNumCommandBytesPerReadByte = 12;
  static constexpr uint8_t kI2cCommandFinishTransaction = 0x87;
  static constexpr uint8_t kFtdiCommandDriveZeroMode = 0x9E;

  zx_status_t WriteIdleToBuf(size_t index, std::vector<uint8_t>* buffer, size_t* bytes_written);
  zx_status_t WriteTransactionStartToBuf(size_t index, std::vector<uint8_t>* buffer,
                                         size_t* bytes_written);
  zx_status_t WriteTransactionEndToBuf(size_t index, std::vector<uint8_t>* buffer,
                                       size_t* bytes_written);
  void WriteI2CByteWriteToBuf(size_t index, uint8_t byte, std::vector<uint8_t>* buffer,
                              size_t* bytes_written);
  void WriteI2CByteReadToBuf(size_t index, bool final_byte, std::vector<uint8_t>* buffer,
                             size_t* bytes_written);

  std::optional<ddk::InitTxn> init_txn_;
  std::atomic_bool enable_thread_started_;
  thrd_t enable_thread_;

  I2cLayout pin_layout_;
  Mpsse mpsse_;
  std::vector<I2cDevice> i2c_devices_;

  fdf::ServerBindingGroup<fuchsia_hardware_i2cimpl::Device> bindings_;
  fdf::OutgoingDirectory outgoing_;

  ddk::MetadataServer<fuchsia_hardware_i2c_businfo::I2CBusMetadata> metadata_server_;
};

}  // namespace ftdi_mpsse

#endif  // SRC_DEVICES_SERIAL_DRIVERS_FTDI_FTDI_I2C_H_
