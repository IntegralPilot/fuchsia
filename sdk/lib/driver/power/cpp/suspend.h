// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_POWER_CPP_SUSPEND_H_
#define LIB_DRIVER_POWER_CPP_SUSPEND_H_

#include <fidl/fuchsia.hardware.power/cpp/fidl.h>
#include <fidl/fuchsia.power.broker/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_base.h>
#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/zx/result.h>

#include <optional>

namespace fdf_power {

// This class is a wrapper for a callback type that must be called into exactly once
// before destruction. It is a move only type.
class Completer {
 public:
  explicit Completer(fit::callback<void()> callback) : callback_(std::move(callback)) {}

  Completer(Completer&& other) noexcept : callback_(std::move(other.callback_)) {
    other.callback_ = std::nullopt;
  }

  Completer(const Completer&) = delete;
  Completer& operator=(const Completer&) = delete;

  ~Completer() {
    ZX_ASSERT_MSG(callback_ == std::nullopt, "Completer was not called before going out of scope.");
  }

  // Calls the wrapped callback function.
  // This method should not be invoked more than once.
  void operator()() {
    ZX_ASSERT_MSG(callback_ != std::nullopt, "Cannot call Completer more than once.");
    auto callback = std::move(callback_.value());
    callback_.reset();
    callback();
  }

 private:
  std::optional<fit::callback<void()>> callback_;
};

// This is the completer for the Suspend operation in |Suspendable|.
class SuspendCompleter final : public Completer {
 public:
  using Completer::Completer;
  using Completer::operator();
};

// This is the completer for the Resume operation in |Suspendable|.
class ResumeCompleter final : public Completer {
 public:
  using Completer::Completer;
  using Completer::operator();
};

// Drivers should use `Suspendable` to implement suspend and resume support for
// their driver. The `fuchsia-suspendable` skill in //src/devices/skills for
// agents should be able to do the basic integration. Information in that skill
// is also a guide for human authors, but a summary is below.
//
// To properly use this mix-in drivers should:
//   * Update their component manifest and add `suspend_enabled: "true"` to the
//     program stanza.
//   * Implement the virtual methods defined here.
//   * Implement `std::optional<fidl::ServerEnd<fuchsia_power_broker::ElementRunner>>
//     take_power_element_runner()` in the driver.
//   * Call `InitializeSuspend` in their start method, after
//     `take_power_element_runner()` can return a valid value.
//
// `InitializeSuspend` does one of two things
//   1) If suspend is not enabled based on `SuspendEnabled`, it returns
//      `zx::ok` immediately.
//   2) If `Driver::take_power_element_runner` returns a value it uses calls
//      to `SetLevel` to levels 0 and 1 to drive calls to `BeforeSuspend` and
//      `AfterResume`, respectively. Otherwise it returns ZX_ERR_UNAVAILABLE.
// The typical implementation for `take_power_element_runner()` returns the
// value from `DriverContext::take_power_element_runner()` from the
// `DriverContext` instance passed to the driver's `Start` hook. The driver
// should take the runner from `DriverContext` during `Start` and store and
// then return it from the driver's `take_power_element_runner` implementation.
template <typename Driver>
class Suspendable {
 public:
  // Interface to be implemented.
  virtual void Suspend(SuspendCompleter completer) = 0;
  virtual void Resume(ResumeCompleter completer) = 0;
  virtual bool SuspendEnabled() = 0;

  explicit Suspendable() : server_(this) {
    static_cast<Driver*>(this)->RegisterInitMethods(
        fit::bind_member(this, &Suspendable::InitializeSuspend));
  }

  // Returns true if
  //   * suspend was enabled and we did one of the following
  //   * we got a value from `Driver::take_power_element_runner`
  // Returns false if suspend was disabled or we didn't get a power element runner.
  bool SuspendActive() { return binding_.has_value(); }

  virtual ~Suspendable() = default;

  zx::result<> InitializeSuspend(async_dispatcher_t* dispatcher, fdf::Namespace& incoming,
                                 std::string_view name) {
    if (!SuspendEnabled()) {
      return zx::ok();
    }

    std::optional<fidl::ServerEnd<fuchsia_power_broker::ElementRunner>> runner =
        static_cast<Driver*>(this)->take_power_element_runner();

    if (!runner.has_value()) {
      return zx::error_result(ZX_ERR_UNAVAILABLE);
    }

    binding_.emplace(dispatcher, std::move(runner.value()), &server_, fidl::kIgnoreBindingClosure);
    return zx::ok();
  }

 private:
  class Server : public fidl::Server<fuchsia_power_broker::ElementRunner> {
   public:
    explicit Server(Suspendable<Driver>* parent) : parent_(parent) {}

   private:
    void SetLevel(SetLevelRequest& request, SetLevelCompleter::Sync& completer) override {
      if (request.level() !=
          static_cast<uint8_t>(fuchsia_hardware_power::FrameworkElementLevels::kOff)) {
        // Log if we receive a level we don't expect. Accept this level though because it provides
        // a transition mechanism for adding new levels without needing to change existing drivers.
        if (request.level() !=
            static_cast<uint8_t>(fuchsia_hardware_power::FrameworkElementLevels::kOn)) {
          fdf::warn("Level {} mapped to 1 since that is the maximum level.", request.level());
        }

        first_activation_occurred_ = true;
        parent_->Resume(
            ResumeCompleter([completer = completer.ToAsync()]() mutable { completer.Reply(); }));
      } else {
        if (first_activation_occurred_) {
          parent_->Suspend(
              SuspendCompleter([completer = completer.ToAsync()]() mutable { completer.Reply(); }));
        } else {
          completer.Reply();
        }
      }
    }
    void handle_unknown_method(
        fidl::UnknownMethodMetadata<fuchsia_power_broker::ElementRunner> metadata,
        fidl::UnknownMethodCompleter::Sync& completer) override {}

    Suspendable<Driver>* parent_;

    // Whether or not the power element has been set to a non-zero level for the first time. This
    // is necessary mostly because currently there is no way to create a power element at level
    // other than zero. We use this to avoid spurious transitions to level zero when the element
    // is created.
    bool first_activation_occurred_ = false;
  };

  Server server_;
  std::optional<fidl::ServerBinding<fuchsia_power_broker::ElementRunner>> binding_;
};

}  // namespace fdf_power

#endif  // LIB_DRIVER_POWER_CPP_SUSPEND_H_
