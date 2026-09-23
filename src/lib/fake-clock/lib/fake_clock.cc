// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.testing.deadline/cpp/wire.h>
#include <fidl/fuchsia.testing/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/port.h>
#include <zircon/compiler.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/port.h>
#include <zircon/utc.h>

#include <atomic>
#include <map>
#include <mutex>
#include <thread>

#include <src/lib/fake-clock/named-timer/named_timer.h>

namespace fake_clock = fuchsia_testing;

namespace {

// Every timer is associated with either the monotonic clock or the boot clock.
// This implementation needs to track that association. This class stores that
// mapping in a thread-safe way.
class ClockOfTimer {
 public:
  // Get the singleton instance of the manager.
  static ClockOfTimer& GetInstance() {
    static ClockOfTimer* instance = new ClockOfTimer();
    return *instance;
  }

  // Thread-safe method to associate a clock type with a handle.
  void SetClockType(zx_handle_t handle, zx_clock_t clock_id) {
    std::lock_guard<std::mutex> guard(mutex_);
    clock_map_[handle] = clock_id;
  }

  // Thread-safe method to retrieve the clock type for a handle.
  // Panics if the handle is not found.
  zx_clock_t GetClockType(zx_handle_t handle) {
    std::lock_guard<std::mutex> guard(mutex_);
    auto it = clock_map_.find(handle);
    if (it != clock_map_.end()) {
      return it->second;
    }
    ZX_PANIC("Could not find clock type for handle %d", handle);
  }

 private:
  // Private constructor to enforce singleton pattern.
  ClockOfTimer() = default;

  std::mutex mutex_;
  std::map<zx_handle_t, zx_clock_t> clock_map_;
};

fidl::UnownedClientEnd<fake_clock::FakeClock> GetService() {
  static std::once_flag svc_connect_once;
  static fidl::ClientEnd<fake_clock::FakeClock> fake_clock;

  // Writing log messages almost anywhere here will crash the program. So,
  // we must do without logging. Also, any errors here will result in bizarre
  // low level stack traces, since the C runtime library calls into this code.
  std::call_once(svc_connect_once, []() {
    if (!fake_clock.is_valid()) {
      zx::result result = component::Connect<fake_clock::FakeClock>();
      if (result.is_error()) {
        FX_PLOGS(ERROR, result.status_value())
            << "Failed to connect to fuchsia.testing.FakeClock service";
      }
      fake_clock = std::move(result.value());
    }
  });
  return fake_clock.borrow();
}

zx::eventpair MakeEvent(zx_time_t deadline, zx_clock_t clock_id) {
  zx::eventpair l, r;
  if (zx_status_t status = zx::eventpair::create(0, &l, &r); status != ZX_OK) {
    ZX_PANIC("%s", zx_status_get_string(status));
  }
  fidl::Status result;
  switch (clock_id) {
    case ZX_CLOCK_MONOTONIC:
      result =
          fidl::WireCall(GetService())->RegisterEventInMonotonic(std::move(r), zx::time(deadline));
      break;
    case ZX_CLOCK_BOOT:
      result =
          fidl::WireCall(GetService())->RegisterEventInBoot(std::move(r), zx::time_boot(deadline));
      break;
    default:
      ZX_PANIC("Unsupported clock ID: %d", clock_id);
  }
  ZX_ASSERT_MSG(result.ok(), "%s", result.FormatDescription().c_str());
  return l;
}

// Manages port keys used by the fake clock implementation. All keys provided by callers to
// zx_object_wait_async() and zx_port_queue() have their keys replaced by a new key allocated
// by this type which is then translated in zx_port_cancel_key() and zx_port_wait() calls.
class PortKeyNamespace {
 public:
  // Allocates a new key backed by a given client key.
  uint64_t AddNewKey(uint64_t client_key);

  // Allocates a new key private to the fake clock library.
  uint64_t AddNewPrivateKey();

  // Maps a key from a Zircon system call back to the client's key and stops tracking
  // the key.
  uint64_t MapAndRemoveKey(uint64_t syscall_key);

 private:
  std::atomic_uint64_t next_available_port_key_{0};
  std::mutex lock_;
  std::unordered_map<uint64_t, uint64_t> syscall_to_client_map_ __TA_GUARDED(lock_);
};

PortKeyNamespace& GetPortKeyNamespace() {
  static auto* port_key_namespace = new PortKeyNamespace;
  return *port_key_namespace;
}

uint64_t PortKeyNamespace::AddNewKey(uint64_t client_key) {
  uint64_t new_key = next_available_port_key_.fetch_add(1);
  std::lock_guard guard(lock_);
  syscall_to_client_map_[new_key] = client_key;
  return new_key;
}

uint64_t PortKeyNamespace::AddNewPrivateKey() {
  // We don't need to track these, the caller is responsible for keeping their use private.
  return next_available_port_key_.fetch_add(1);
}

uint64_t PortKeyNamespace::MapAndRemoveKey(uint64_t syscall_key) {
  std::lock_guard guard(lock_);
  auto it = syscall_to_client_map_.find(syscall_key);
  ZX_ASSERT(it != syscall_to_client_map_.end());
  uint64_t client_key = it->second;
  syscall_to_client_map_.erase(it);
  return client_key;
}

void TranslateKeyInPacket(zx_port_packet& packet) {
  packet.key = GetPortKeyNamespace().MapAndRemoveKey(packet.key);
}

}  // namespace

__EXPORT zx_status_t zx_futex_wait(const zx_futex_t* value_ptr, zx_futex_t current_value,
                                   zx_handle_t new_futex_owner, zx_time_t deadline) {
  ZX_ASSERT_MSG(deadline == ZX_TIME_INFINITE,
                "zx_futex_wait with deadline is currently supported by FakeClock library");
  return _zx_futex_wait(value_ptr, current_value, new_futex_owner, deadline);
}

__EXPORT zx_status_t zx_channel_call(zx_handle_t handle, uint32_t options, zx_time_t deadline,
                                     const zx_channel_call_args_t* args, uint32_t* actual_bytes,
                                     uint32_t* actual_handles) {
  // TODO(brunodalbo) There may be a way to get channel_call working if we create a temporary
  // channel and an auxiliary thread, but looks like most channel_call call sites don't define
  // deadlines.
  ZX_ASSERT_MSG(deadline == ZX_TIME_INFINITE,
                "zx_channel_call with deadline is not yet supported by FakeClock library");
  return _zx_channel_call(handle, options, deadline, args, actual_bytes, actual_handles);
}

__EXPORT zx_time_t zx_clock_get_monotonic() {
  const fidl::WireResult result = fidl::WireCall(GetService())->Get();
  ZX_ASSERT_MSG(result.ok(), "%s", result.FormatDescription().c_str());
  return result.value().monotonic_time.get();
}

__EXPORT zx_time_t zx_clock_get_boot() {
  const fidl::WireResult result = fidl::WireCall(GetService())->Get();
  ZX_ASSERT_MSG(result.ok(), "%s", result.FormatDescription().c_str());
  return result.value().boot_time.get();
}

__EXPORT zx_time_t zx_deadline_after(zx_duration_t duration) {
  return zx_time_add_duration(zx_clock_get_monotonic(), duration);
}

__EXPORT zx_status_t zx_nanosleep(zx_time_t deadline) {
  zx::eventpair e = MakeEvent(deadline, ZX_CLOCK_MONOTONIC);
  if (zx_status_t status =
          _zx_object_wait_one(e.get(), ZX_EVENTPAIR_SIGNALED, ZX_TIME_INFINITE, nullptr) != ZX_OK) {
    ZX_PANIC("%s", zx_status_get_string(status));
  }
  return ZX_OK;
}

// wait_one is implemented by making it a wait_many on an infinite deadline with two items: one is
// the original handle+signals, the other is the eventpair created from the fake-clock service.
__EXPORT zx_status_t zx_object_wait_one(zx_handle_t handle, zx_signals_t signals,
                                        zx_time_t deadline, zx_signals_t* observed) {
  if (deadline == ZX_TIME_INFINITE || deadline == 0) {
    return _zx_object_wait_one(handle, signals, deadline, observed);
  }

  zx::eventpair e = MakeEvent(deadline, ZX_CLOCK_MONOTONIC);
  zx_wait_item_t items[] = {
      {
          .handle = e.get(),
          .waitfor = ZX_EVENTPAIR_SIGNALED,
      },
      {
          .handle = handle,
          .waitfor = signals,
      },
  };

  zx_status_t status = _zx_object_wait_many(items, 2, ZX_TIME_INFINITE);
  if (observed) {
    *observed = items[1].pending;
  }
  if (status != ZX_OK) {
    return status;
  }
  if ((items[0].pending & ZX_EVENTPAIR_SIGNALED) != 0) {
    return ZX_ERR_TIMED_OUT;
  }
  return ZX_OK;
}

// wait_many is implemented by adding an extra eventpair handle extracted from fake-clock to the
// wait list, and changing the deadline to infinite. If the number of items on the wait is already
// ZX_WAIT_MANY_MAX_ITEMS (meaning we can't add an extra item), we create a port instead and
// register all the wait items to it.
__EXPORT zx_status_t zx_object_wait_many(zx_wait_item_t* items, size_t num_items,
                                         zx_time_t deadline) {
  if (deadline == ZX_TIME_INFINITE || deadline == 0 || num_items > ZX_WAIT_MANY_MAX_ITEMS) {
    return _zx_object_wait_many(items, num_items, deadline);
  }
  if (num_items == ZX_WAIT_MANY_MAX_ITEMS) {
    // can't add a new item, we need to build a port and wait on it.
    zx::port port;
    if (zx_status_t status = zx::port::create(0, &port); status != ZX_OK) {
      ZX_PANIC("%s", zx_status_get_string(status));
    }
    for (size_t i = 0; i < num_items; i++) {
      if (zx_status_t status =
              _zx_object_wait_async(items[i].handle, port.get(), i, items[i].waitfor, 0);
          status != ZX_OK) {
        return status;
      }
    }
    zx::eventpair event = MakeEvent(deadline, ZX_CLOCK_MONOTONIC);
    if (zx_status_t status =
            _zx_object_wait_async(event.get(), port.get(), num_items, ZX_EVENTPAIR_SIGNALED, 0);
        status != ZX_OK) {
      ZX_PANIC("%s", zx_status_get_string(status));
    }

    auto update_item = [&items, num_items](const zx_port_packet& packet) {
      if (packet.key == num_items) {
        if (packet.signal.observed & ZX_EVENTPAIR_SIGNALED) {
          return true;
        }
      } else {
        items[packet.key].pending = packet.signal.observed;
      }
      return false;
    };

    zx_port_packet_t packet;
    if (zx_status_t status = _zx_port_wait(port.get(), ZX_TIME_INFINITE, &packet);
        status != ZX_OK) {
      return status;
    }
    // update_item will return true if the first packet out of the port is a timeout.
    if (update_item(packet)) {
      return ZX_ERR_TIMED_OUT;
    }
    // many things may have happened at once, how we just keep polling the port with a zero deadline
    // and updating the items
    while (_zx_port_wait(port.get(), 0u, &packet) == ZX_OK) {
      if (update_item(packet)) {
        break;
      }
    }
    return ZX_OK;
  }
  // we can just add an extra item, but we'll need to copy all the wait items
  zx_wait_item_t tmp[ZX_WAIT_MANY_MAX_ITEMS];
  std::copy_n(items, num_items, tmp);
  zx::eventpair event = MakeEvent(deadline, ZX_CLOCK_MONOTONIC);
  tmp[num_items].pending = 0;
  tmp[num_items].waitfor = ZX_EVENTPAIR_SIGNALED;
  tmp[num_items].handle = event.get();
  zx_status_t status = _zx_object_wait_many(tmp, num_items + 1, ZX_TIME_INFINITE);
  // copy everything back:
  std::copy_n(tmp, num_items, items);
  if (status != ZX_OK) {
    return status;
  }
  if ((tmp[num_items].pending & ZX_EVENTPAIR_SIGNALED) != 0) {
    return ZX_ERR_TIMED_OUT;
  }

  return ZX_OK;
}

__EXPORT zx_status_t zx_object_wait_async(zx_handle_t handle, zx_handle_t port, uint64_t key,
                                          uint32_t signals, uint32_t options) {
  // Allocate a key for |key| that is unique within this process and register on that.
  uint64_t syscall_key = GetPortKeyNamespace().AddNewKey(key);
  return _zx_object_wait_async(handle, port, syscall_key, signals, options);
}

__EXPORT zx_status_t zx_port_cancel_key(zx_handle_t handle, uint32_t options, uint64_t key) {
  uint64_t syscall_key = GetPortKeyNamespace().MapAndRemoveKey(key);
  return _zx_port_cancel_key(handle, options, syscall_key);
}

__EXPORT zx_status_t zx_port_queue(zx_handle_t handle, const zx_port_packet_t* packet) {
  uint64_t syscall_key = GetPortKeyNamespace().AddNewKey(packet->key);
  zx_port_packet_t translated_packet = *packet;
  translated_packet.key = syscall_key;
  return _zx_port_queue(handle, &translated_packet);
}

// port_wait adds an extra fake-clock eventpair handle to the port and changes the deadline to
// ZX_TIME_INFINITE.
__EXPORT zx_status_t zx_port_wait(zx_handle_t handle, zx_time_t deadline,
                                  zx_port_packet_t* packet) {
  if (deadline == ZX_TIME_INFINITE) {
    zx_status_t status = _zx_port_wait(handle, deadline, packet);
    if (status == ZX_OK) {
      TranslateKeyInPacket(*packet);
    }
    return status;
  }

  zx::eventpair event = MakeEvent(deadline, ZX_CLOCK_MONOTONIC);
  uint64_t private_syscall_key = GetPortKeyNamespace().AddNewPrivateKey();
  if (zx_status_t status =
          _zx_object_wait_async(event.get(), handle, private_syscall_key, ZX_EVENTPAIR_SIGNALED, 0);
      status != ZX_OK) {
    ZX_PANIC("%s", zx_status_get_string(status));
  }
  zx_port_packet_t tmp;
  zx_status_t status = _zx_port_wait(handle, ZX_TIME_INFINITE, &tmp);
  // Always cancel the wait.
  _zx_port_cancel_key(handle, 0u, private_syscall_key);
  if (status != ZX_OK) {
    return status;
  }
  if (tmp.type == ZX_PKT_TYPE_SIGNAL_ONE && tmp.key == private_syscall_key &&
      tmp.signal.observed == ZX_EVENTPAIR_SIGNALED) {
    return ZX_ERR_TIMED_OUT;
  }
  TranslateKeyInPacket(tmp);
  *packet = tmp;
  return ZX_OK;
}

// timer_create changes the type of returned handle from an actual timer to one side of an eventpair
// created by fake-clock. It relies on the fact that ZX_EVENTPAIR_SIGNALED is the same bit as
// ZX_TIMER_SIGNALED, meaning unless clients are inspecting the handle type, they shouldn't be able
// to tell the difference.
__EXPORT zx_status_t zx_timer_create(uint32_t options, zx_clock_t clock_id, zx_handle_t* out) {
  // We're replacing a timer with an event, and shamelessly using the fact that
  // ZX_EVENTPAIR_SIGNALED and ZX_TIMER_SIGNAL collide, this assertion protects that assumption more
  // strongly.
  static_assert(ZX_EVENTPAIR_SIGNALED == ZX_TIMER_SIGNALED);
  if (clock_id != ZX_CLOCK_MONOTONIC && clock_id != ZX_CLOCK_BOOT) {
    // NOTE: _zx_timer_create will just fail according to the docs.
    return _zx_timer_create(options, clock_id, out);
  }
  // Create an event with infinite deadline and return that instead of a timer handle
  *out = MakeEvent(ZX_TIME_INFINITE, clock_id).release();
  ClockOfTimer::GetInstance().SetClockType(*out, clock_id);
  return ZX_OK;
}

__EXPORT zx_status_t zx_timer_set(zx_handle_t handle, zx_time_t deadline, zx_duration_t slack) {
  zx_clock_t clock_id = ClockOfTimer::GetInstance().GetClockType(handle);

  zx::eventpair e;
  if (zx_status_t status = zx::unowned_eventpair(handle)->duplicate(ZX_RIGHT_SAME_RIGHTS, &e);
      status != ZX_OK) {
    return status;
  }

  switch (clock_id) {
    case ZX_CLOCK_MONOTONIC: {
      auto result = fidl::WireCall(GetService())
                        ->RescheduleEventInMonotonic(std::move(e), zx::time(deadline));
      if (!result.ok()) {
        return result.status();
      }
      if (result->is_error()) {
        return result->error_value();
      }
      break;
    }
    case ZX_CLOCK_BOOT: {
      auto result = fidl::WireCall(GetService())
                        ->RescheduleEventInBoot(std::move(e), zx::time_boot(deadline));
      if (!result.ok()) {
        return result.status();
      }
      if (result->is_error()) {
        return result->error_value();
      }
      break;
    }
  }
  return ZX_OK;
}

__EXPORT zx_status_t zx_timer_cancel(zx_handle_t handle) {
  zx::eventpair e;
  if (zx_status_t status = zx::unowned_eventpair(handle)->duplicate(ZX_RIGHT_SAME_RIGHTS, &e);
      status != ZX_OK) {
    return status;
  }
  const fidl::WireResult result = fidl::WireCall(GetService())->CancelEvent(std::move(e));
  ZX_ASSERT_MSG(result.ok(), "%s", result.FormatDescription().c_str());
  return ZX_OK;
}

__EXPORT bool create_named_deadline(char* component, size_t component_len, char* code,
                                    size_t code_len, zx_time_t duration, zx_time_t* out) {
  const fidl::WireResult result =
      fidl::WireCall(GetService())
          ->CreateNamedDeadlineInMonotonic(
              {
                  .component_id = fidl::StringView::FromExternal(component, component_len),
                  .code = fidl::StringView::FromExternal(code, code_len),
              },
              duration);
  ZX_ASSERT_MSG(result.ok(), "%s", result.FormatDescription().c_str());
  *out = result.value().deadline.get();
  return true;
}

__EXPORT zx_handle_t zx_utc_reference_get() {
#ifdef FAKE_CLOCK_ALLOW_UTC
  return _zx_utc_reference_get();
#else
  FX_LOGS(FATAL) << "UTC clock may interact in unexpected ways with the fake-clock library. "
                 << "See //src/lib/fake-clock/README.md for ways to fix this.";
  return ZX_HANDLE_INVALID;
#endif  // FAKE_CLOCK_ALLOW_UTC
}
