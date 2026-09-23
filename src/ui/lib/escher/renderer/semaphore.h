// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_LIB_ESCHER_RENDERER_SEMAPHORE_H_
#define SRC_UI_LIB_ESCHER_RENDERER_SEMAPHORE_H_

#include "src/lib/fxl/macros.h"
#include "src/ui/lib/escher/base/reffable.h"

#include <vulkan/vulkan.hpp>

namespace escher {

class SemaphorePool;

class Semaphore;
typedef fxl::RefPtr<Semaphore> SemaphorePtr;

class Semaphore : public Reffable {
 public:
  explicit Semaphore(vk::Device device, SemaphorePool* pool = nullptr);
  ~Semaphore() override;

  // Convenient.
  static SemaphorePtr New(vk::Device device, SemaphorePool* pool = nullptr);

  vk::Semaphore vk_semaphore() const { return value_; }

  bool is_imported() const { return is_imported_; }
  void set_is_imported(bool is_imported) { is_imported_ = is_imported; }

 protected:
  bool OnZeroRefCount() override;

 private:
  friend class SemaphorePool;

  vk::Device device_;
  vk::Semaphore value_;
  SemaphorePool* pool_ = nullptr;
  bool is_imported_ = false;

  FXL_DISALLOW_COPY_AND_ASSIGN(Semaphore);
};

}  // namespace escher

#endif  // SRC_UI_LIB_ESCHER_RENDERER_SEMAPHORE_H_
