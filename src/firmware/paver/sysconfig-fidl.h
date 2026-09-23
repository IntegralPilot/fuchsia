// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_FIRMWARE_PAVER_SYSCONFIG_FIDL_H_
#define SRC_FIRMWARE_PAVER_SYSCONFIG_FIDL_H_

#include <fidl/fuchsia.io/cpp/markers.h>
#include <fidl/fuchsia.paver/cpp/wire.h>
#include <lib/zx/channel.h>

#include "src/firmware/paver/block-devices.h"
#include "src/firmware/paver/config.h"
#include "src/firmware/paver/partition-client.h"
#include "src/firmware/paver/paver-context.h"

namespace paver {

class Sysconfig : public fidl::WireServer<fuchsia_paver::Sysconfig> {
 public:
  explicit Sysconfig(std::unique_ptr<PartitionClient> client) : partitioner_(std::move(client)) {}

  static void Bind(async_dispatcher_t* dispatcher, const BlockDevices& devices,
                   fidl::ClientEnd<fuchsia_io::Directory> svc_root, const PaverConfig& config,
                   std::shared_ptr<Context> context,
                   fidl::ServerEnd<fuchsia_paver::Sysconfig> server);

  void Read(ReadCompleter::Sync& completer) override;

  void Write(WriteRequestView request, WriteCompleter::Sync& completer) override;

  void GetPartitionSize(GetPartitionSizeCompleter::Sync& completer) override;

  void Flush(FlushCompleter::Sync& completer) override;

  void Wipe(WipeCompleter::Sync& completer) override;

 private:
  std::unique_ptr<PartitionClient> partitioner_;
};

}  // namespace paver

#endif  // SRC_FIRMWARE_PAVER_SYSCONFIG_FIDL_H_
