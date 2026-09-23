// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FDF_CPP_WAKE_VECTOR_H_
#define LIB_FDF_CPP_WAKE_VECTOR_H_

#include <lib/fdf/cpp/dispatcher.h>
#include <lib/zx/handle.h>
#include <lib/zx/result.h>

#include <utility>

#if FUCHSIA_API_LEVEL_AT_LEAST(32)

namespace fdf {

// RAII class that manages a registered wake vector.
// When this object is destroyed, it automatically unregisters the wake vector.
class WakeVectorRegistration final {
 public:
  // Creates an empty, inactive registration.
  WakeVectorRegistration() = default;

  // Not copyable.
  WakeVectorRegistration(const WakeVectorRegistration&) = delete;
  WakeVectorRegistration& operator=(const WakeVectorRegistration&) = delete;

  // Move-constructable.
  WakeVectorRegistration(WakeVectorRegistration&& other) noexcept
      : dispatcher_(std::exchange(other.dispatcher_, nullptr)),
        handle_(std::exchange(other.handle_, ZX_HANDLE_INVALID)),
        signals_(std::exchange(other.signals_, 0)) {}

  // Move-assignable.
  WakeVectorRegistration& operator=(WakeVectorRegistration&& other) noexcept {
    if (this != &other) {
      Reset();
      dispatcher_ = std::exchange(other.dispatcher_, nullptr);
      handle_ = std::exchange(other.handle_, ZX_HANDLE_INVALID);
      signals_ = std::exchange(other.signals_, 0);
    }
    return *this;
  }

  // Automatically unregisters the wake vector.
  ~WakeVectorRegistration() { Reset(); }

  // Factory to register and return a WakeVectorRegistration.
  static zx::result<WakeVectorRegistration> Create(const UnownedDispatcher& dispatcher,
                                                   const zx::unowned_handle& handle,
                                                   zx_signals_t signals) {
    zx_status_t status =
        fdf_dispatcher_register_wake_vector(dispatcher->get(), handle->get(), signals);
    if (status != ZX_OK) {
      return zx::error(status);
    }
    return zx::ok(WakeVectorRegistration(dispatcher->get(), handle->get(), signals));
  }

  // Overload for owned dispatchers.
  static zx::result<WakeVectorRegistration> Create(const Dispatcher& dispatcher,
                                                   const zx::unowned_handle& handle,
                                                   zx_signals_t signals) {
    return Create(dispatcher.borrow(), handle, signals);
  }

  // Manually unregisters the wake vector before the object's destruction.
  void Reset() {
    if (dispatcher_) {
      fdf_dispatcher_unregister_wake_vector(dispatcher_, handle_, signals_);
      dispatcher_ = nullptr;
      handle_ = ZX_HANDLE_INVALID;
      signals_ = 0;
    }
  }

  // Check if this object holds an active registration.
  explicit operator bool() const { return dispatcher_ != nullptr; }

 private:
  WakeVectorRegistration(fdf_dispatcher_t* dispatcher, zx_handle_t handle, zx_signals_t signals)
      : dispatcher_(dispatcher), handle_(handle), signals_(signals) {}

  fdf_dispatcher_t* dispatcher_ = nullptr;
  zx_handle_t handle_ = ZX_HANDLE_INVALID;
  zx_signals_t signals_ = 0;
};

}  // namespace fdf

#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)

#endif  // LIB_FDF_CPP_WAKE_VECTOR_H_
