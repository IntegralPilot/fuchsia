// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYSMEM_SERVER_APP_H_
#define SRC_SYSMEM_SERVER_APP_H_

#include <memory>

#include "src/lib/fxl/macros.h"
#include "src/sysmem/server/sysmem.h"

class App {
 public:
  explicit App(async_dispatcher_t* dispatcher);
  ~App();

 private:
  async_dispatcher_t* dispatcher_ = nullptr;

  std::unique_ptr<sysmem_service::Sysmem> device_;

  FXL_DISALLOW_COPY_AND_ASSIGN(App);
};

#endif  // SRC_SYSMEM_SERVER_APP_H_
