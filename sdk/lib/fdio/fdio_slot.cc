// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sdk/lib/fdio/fdio_slot.h"

#include "sdk/lib/fdio/internal.h"

fbl::RefPtr<fdio> fdio_slot::get() {
  fbl::RefPtr<fdio_t>* ptr = std::get_if<fbl::RefPtr<fdio_t>>(&inner_);
  if (ptr != nullptr) {
    return *ptr;
  }
  return nullptr;
}

fbl::RefPtr<fdio> fdio_slot::release() {
  fbl::RefPtr<fdio_t>* ptr = std::get_if<fbl::RefPtr<fdio_t>>(&inner_);
  if (ptr != nullptr) {
    fbl::RefPtr<fdio> io = std::move(*ptr);
    inner_ = available{};
    return io;
  }
  return nullptr;
}

bool fdio_slot::try_set(fbl::RefPtr<fdio> io) {
  if (std::holds_alternative<available>(inner_)) {
    inner_ = std::move(io);
    return true;
  }
  return false;
}

fbl::RefPtr<fdio> fdio_slot::replace(fbl::RefPtr<fdio> io) {
  auto previous = std::exchange(inner_, std::move(io));
  fbl::RefPtr<fdio_t>* ptr = std::get_if<fbl::RefPtr<fdio_t>>(&previous);
  if (ptr != nullptr) {
    return std::move(*ptr);
  }
  return nullptr;
}

std::optional<void (fdio_slot::*)()> fdio_slot::try_reserve() {
  if (std::holds_alternative<available>(inner_)) {
    inner_ = reserved{};
    return &fdio_slot::release_reservation;
  }
  return std::nullopt;
}

bool fdio_slot::try_fill(fbl::RefPtr<fdio> io) {
  if (std::holds_alternative<reserved>(inner_)) {
    inner_ = std::move(io);
    return true;
  }
  return false;
}

bool fdio_slot::allocated() const { return !std::holds_alternative<available>(inner_); }

void fdio_slot::release_reservation() {
  if (std::holds_alternative<reserved>(inner_)) {
    inner_ = available{};
  }
}
