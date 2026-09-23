// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_FLATLAND_HANGING_GET_HELPER_H_
#define SRC_UI_SCENIC_LIB_FLATLAND_HANGING_GET_HELPER_H_

#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/async/dispatcher.h>
#include <lib/fidl/cpp/clone.h>
#include <lib/fidl/cpp/comparison.h>
#include <lib/fit/function.h>
#include <lib/syslog/cpp/macros.h>

#include <mutex>
#include <optional>
#include <type_traits>
#include <utility>

namespace flatland {

namespace internal {

template <typename T, typename = void>
struct HasEqualityOperator : std::false_type {};

template <typename T>
struct HasEqualityOperator<
    T, std::void_t<decltype(std::declval<const T&>() == std::declval<const T&>())>>
    : std::true_type {};

template <typename T, typename = void>
struct HasFidlEqualityOperator : std::false_type {};

template <typename T>
struct HasFidlEqualityOperator<T, std::void_t<decltype(std::declval<::fidl::Equality<T>>()(
                                      std::declval<const T&>(), std::declval<const T&>()))>>
    : std::true_type {};

template <typename T>
bool DataEquals(const T& a, const T& b) {
  if constexpr (HasEqualityOperator<T>::value) {
    return a == b;
  } else if constexpr (std::is_same_v<T, fuchsia_ui_views::ViewRef>) {
    if (!a.reference().is_valid() && !b.reference().is_valid()) {
      return true;
    }
    if (!a.reference().is_valid() || !b.reference().is_valid()) {
      return false;
    }
    zx_info_handle_basic_t a_info{}, b_info{};
    a.reference().get_info(ZX_INFO_HANDLE_BASIC, &a_info, sizeof(a_info), nullptr, nullptr);
    b.reference().get_info(ZX_INFO_HANDLE_BASIC, &b_info, sizeof(b_info), nullptr, nullptr);
    return a_info.koid != ZX_KOID_INVALID && a_info.koid == b_info.koid;
  } else if constexpr (HasFidlEqualityOperator<T>::value) {
    return fidl::Equals(a, b);
  } else {
    static_assert(sizeof(T) == 0, "Type must support either operator== or fidl::Equals");
    return false;
  }
}

template <typename T, typename = void>
struct HasCloneMethod : std::false_type {};

template <typename T>
struct HasCloneMethod<T, std::void_t<decltype(std::declval<const T&>().Clone(std::declval<T*>()))>>
    : std::true_type {};

template <typename T>
T DataClone(const T& val) {
  if constexpr (std::is_copy_constructible_v<T>) {
    return val;
  } else if constexpr (std::is_same_v<T, fuchsia_ui_views::ViewRef>) {
    fuchsia_ui_views::ViewRef out;
    if (val.reference().is_valid()) {
      zx::eventpair handle;
      zx_status_t status = val.reference().duplicate(ZX_RIGHT_SAME_RIGHTS, &handle);
      FX_DCHECK(status == ZX_OK);
      out.reference(std::move(handle));
    }
    return out;
  } else if constexpr (HasCloneMethod<T>::value) {
    T out = T();
    fidl::Clone(val, &out);
    return out;
  } else {
    static_assert(sizeof(T) == 0,
                  "Type must be copy-constructible, ViewRef, or support fidl::Clone");
    return {};
  }
}

}  // namespace internal

/// A helper class for managing [hanging get
/// semantics](https://fuchsia.dev/fuchsia-src/development/api/fidl.md#delay-responses-using-hanging-gets).
/// It responds with the most recently updated value.
///
/// For each hanging get method in a FIDL interface, like GetData() -> ( Data response ), create one
/// of these classes. Any time the response should change, call Update(Data x). Any time the client
/// calls GetFoo(), set the callback on this helper. Once the callback has been set and the data has
/// been updated, the callback will be triggered with the new data.
///
/// Each callback will only be triggered once. Each Update will only trigger, at most, a single
/// callback. Update(Data x) is idempotent. Calling it with the same value will not trigger a new
/// execution of a registered callback, nor will it remove the registered callback.
template <class Data>
class HangingGetHelper {
 public:
  using Callback = fit::function<void(Data)>;

  HangingGetHelper() = default;

  void Update(Data data) {
    std::lock_guard<std::mutex> guard(mutex_);

    if (last_data_ && internal::DataEquals(last_data_.value(), data)) {
      return;
    }

    data_ = std::move(data);
    SendIfReady();
  }

  void SetCallback(Callback callback) {
    std::lock_guard<std::mutex> guard(mutex_);

    callback_ = std::move(callback);
    SendIfReady();
  }

  bool HasPendingCallback() {
    std::lock_guard<std::mutex> guard(mutex_);
    return static_cast<bool>(callback_);
  }

 private:
  void SendIfReady() {
    if (data_ && callback_) {
      last_data_ = internal::DataClone(data_.value());

      callback_(std::move(data_.value()));

      data_.reset();
      callback_ = nullptr;
    }
  }

  std::mutex mutex_;
  std::optional<Data> data_;
  std::optional<Data> last_data_;
  Callback callback_;
};

}  // namespace flatland

#endif  // SRC_UI_SCENIC_LIB_FLATLAND_HANGING_GET_HELPER_H_
