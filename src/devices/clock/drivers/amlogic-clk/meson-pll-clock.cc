// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "meson-pll-clock.h"

namespace amlogic_clock {

void MesonPllClock::Init() {
  const hhi_pll_rate_t* rate_table = nullptr;
  size_t rate_table_size = 0;

  if (meson_hiudev_) {
    rate_table = meson_hiudev_->GetRateTable();
    rate_table_size = meson_hiudev_->GetRateTableSize();
  } else {
    s905d2_pll_init_etc(hiudev_, &pll_, pll_num_);

    rate_table = s905d2_pll_get_rate_table(pll_num_);
    rate_table_size = s905d2_get_rate_table_count(pll_num_);
  }

  // Make sure that the rate table is sorted in strictly ascending order.
  for (size_t i = 0; i < rate_table_size - 1; i++) {
    ZX_ASSERT(rate_table[i].rate < rate_table[i + 1].rate);
  }
}

zx_status_t MesonPllClock::SetRate(const uint32_t hz) {
  if (meson_hiudev_) {
    return meson_hiudev_->SetRate(hz);
  }

  return s905d2_pll_set_rate(&pll_, hz);
}

zx::result<uint64_t> MesonPllClock::QuerySupportedRate(const uint64_t max_rate) {
  // Find the largest rate that does not exceed `max_rate`

  // Start by getting the rate tables.
  const hhi_pll_rate_t* rate_table = nullptr;
  size_t rate_table_size = 0;
  const hhi_pll_rate_t* best_rate = nullptr;

  if (meson_hiudev_) {
    rate_table = meson_hiudev_->GetRateTable();
    rate_table_size = meson_hiudev_->GetRateTableSize();
  } else {
    rate_table = s905d2_pll_get_rate_table(pll_num_);
    rate_table_size = s905d2_get_rate_table_count(pll_num_);
  }

  // The rate table is already sorted in ascending order so pick the largest
  // element that does not exceed max_rate.
  for (size_t i = 0; i < rate_table_size; i++) {
    if (rate_table[i].rate <= max_rate) {
      best_rate = &rate_table[i];
    } else {
      break;
    }
  }

  if (best_rate == nullptr) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  return zx::ok(best_rate->rate);
}

zx::result<uint64_t> MesonPllClock::GetRate() { return zx::error(ZX_ERR_NOT_SUPPORTED); }

zx_status_t MesonPllClock::Toggle(const bool enable) {
  if (enable) {
    if (meson_hiudev_) {
      return meson_hiudev_->Enable();
    }
    return s905d2_pll_ena(&pll_);
  }
  if (meson_hiudev_) {
    meson_hiudev_->Disable();
  } else {
    s905d2_pll_disable(&pll_);
  }
  return ZX_OK;
}

}  // namespace amlogic_clock
