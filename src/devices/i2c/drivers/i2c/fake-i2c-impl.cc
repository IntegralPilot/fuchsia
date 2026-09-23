// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/i2c/drivers/i2c/fake-i2c-impl.h"

namespace i2c {
const FakeI2cImpl::OnTransact FakeI2cImpl::kDefaultOnTransact =
    [](i2c::FakeI2cImpl::TransactRequestView request, fdf::Arena& arena,
       i2c::FakeI2cImpl::TransactCompleter::Sync& completer) {
      completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
    };

void FakeI2cImpl::GetMaxTransferSize(fdf::Arena& arena,
                                     GetMaxTransferSizeCompleter::Sync& completer) {
  completer.buffer(arena).ReplySuccess(max_transfer_size_);
}

void FakeI2cImpl::SetBitrate(SetBitrateRequestView request, fdf::Arena& arena,
                             SetBitrateCompleter::Sync& completer) {
  completer.buffer(arena).ReplySuccess();
}

void FakeI2cImpl::Transact(TransactRequestView request, fdf::Arena& arena,
                           TransactCompleter::Sync& completer) {
  on_transact_(request, arena, completer);
}

fuchsia_hardware_i2cimpl::Service::InstanceHandler FakeI2cImpl::CreateInstanceHandler() {
  return fuchsia_hardware_i2cimpl::Service::InstanceHandler(
      {.device = bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->get(),
                                         fidl::kIgnoreBindingClosure)});
}
}  // namespace i2c
