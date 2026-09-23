// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_NAND_DRIVERS_NAND_NAND_H_
#define SRC_DEVICES_NAND_DRIVERS_NAND_NAND_H_

#include <fidl/fuchsia.boot.metadata/cpp/fidl.h>
#include <fuchsia/hardware/nand/cpp/banjo.h>
#include <fuchsia/hardware/rawnand/cpp/banjo.h>
#include <lib/driver/compat/cpp/banjo_server.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/metadata/cpp/metadata_server.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/inspect/cpp/vmo/types.h>
#include <lib/operation/nand.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <threads.h>
#include <zircon/types.h>

#include <ddktl/device.h>
#include <fbl/condition_variable.h>
#include <fbl/intrusive_double_list.h>

#include "src/devices/nand/drivers/nand/read_cache.h"

namespace nand {

using Transaction = nand::BorrowedOperation<>;

class NandDriver : public fdf::DriverBase2, public ddk::NandProtocol<NandDriver> {
 public:
  static constexpr std::string_view kDriverName = "nand";
  static constexpr std::string_view kChildNodeName = "nand";

  // Based on field metrics, this is estimated to recover for 99.5% of the failed reads that recover
  // at 100 retries while reducing the read disturb on the pages 10x to prevent tipping into
  // undetected ECC failures which makes debugging and triage difficult.
  static constexpr size_t kNandReadRetries = 10;

  explicit NandDriver() : fdf::DriverBase2(kDriverName) {}

  // fdf::DriverBase2 implementation.
  zx::result<> Start(fdf::DriverContext context) override;
  void Stop(fdf::StopCompleter completer) override;

  // Nand protocol implementation.
  void NandQuery(nand_info_t* info_out, size_t* nand_op_size_out);
  void NandQueue(nand_operation_t* op, nand_queue_callback completion_cb, void* cookie);
  zx_status_t NandGetFactoryBadBlockList(uint32_t* bad_blocks, size_t bad_block_len,
                                         size_t* num_bad_blocks);

 protected:
  std::optional<inspect::ComponentInspector>& component_inspector() { return component_inspector_; }

 private:
  // Maps the data and oob vmos from the specified |nand_op| into memory.
  zx_status_t MapVmos(const nand_operation_t& nand_op, fzl::VmoMapper* data, uint8_t** vaddr_data,
                      fzl::VmoMapper* oob, uint8_t** vaddr_oob);

  // Calls controller specific read function.
  // data, oob: pointers to user out-of-band data and data buffers.
  // nand_page : NAND page address to read.
  // ecc_correct : Number of ecc corrected bitflips (< 0 indicates
  // ecc could not correct all bitflips - caller needs to check that).
  // retries : Retry logic may not be needed.
  zx_status_t ReadPage(uint8_t* data, uint8_t* oob, uint32_t nand_page, uint32_t* corrected_bits,
                       size_t retries);

  zx_status_t EraseOp(nand_operation_t* nand_op);
  zx_status_t ReadOp(nand_operation_t* nand_op);
  zx_status_t WriteOp(nand_operation_t* nand_op);

  void PerformTransaction(Transaction transaction);

  ddk::RawNandProtocolClient raw_nand_;

  nand_info_t nand_info_;
  uint32_t num_nand_pages_;

  inspect::Node root_;

  // Track number of bit flips in each read attempt, ECC failures records max ECC plus one.
  inspect::LinearUintHistogram read_ecc_bit_flips_;

  // Number of read attempts until success. Failures will populate as maxint to go in the overflow
  // bucket.
  inspect::ExponentialUintHistogram read_attempts_;

  // Count internal read failures
  inspect::UintProperty read_internal_failure_;

  // Count read failures where all retries are exhausted.
  inspect::UintProperty read_failure_;

  // Cache for recent reads that came close to failure.
  std::unique_ptr<ReadCache> dangerous_reads_cache_ = nullptr;

  // If a read call doesn't want the oob, store it here instead to facilitate caching.
  std::unique_ptr<uint8_t[]> oob_buffer_ = nullptr;

  fdf::SynchronizedDispatcher transaction_performer_dispatcher_;

  // Completed by `transaction_performer_dispatcher_` when the dispatcher is shutdown.
  std::optional<fdf::StopCompleter> prepare_stop_completer_;

  compat::BanjoServer nand_server_{ZX_PROTOCOL_NAND, this, &nand_protocol_ops_};
  compat::SyncInitializedDeviceServer compat_server_;
  fidl::ClientEnd<fuchsia_driver_framework::NodeController> child_;
  std::optional<inspect::ComponentInspector> component_inspector_;
  fdf_metadata::MetadataServer<fuchsia_boot_metadata::PartitionMap> partition_map_metadata_server_;
};

}  // namespace nand

#endif  // SRC_DEVICES_NAND_DRIVERS_NAND_NAND_H_
