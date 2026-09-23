// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_TESTS_EVAL_DEVICETREE_FAKE_BOARD_FAKE_BOARD_H_
#define SRC_DEVICES_TESTS_EVAL_DEVICETREE_FAKE_BOARD_FAKE_BOARD_H_

#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/devicetree/manager/manager.h>
#include <lib/zx/result.h>

namespace devicetree_evaluation {

class FakeBoard : public fdf::DriverBase2 {
 public:
  FakeBoard() : fdf::DriverBase2("fake-board") {}
  zx::result<> Start(fdf::DriverContext context) override;

 private:
  fidl::SyncClient<fuchsia_driver_framework::Node> node_;
};

}  // namespace devicetree_evaluation

#endif  // SRC_DEVICES_TESTS_EVAL_DEVICETREE_FAKE_BOARD_FAKE_BOARD_H_
