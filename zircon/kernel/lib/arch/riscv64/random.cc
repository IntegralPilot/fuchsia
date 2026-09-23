// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/random.h>
#include <zircon/assert.h>

namespace arch {

template <bool Reseed>
bool Random<Reseed>::Supported() {
  // TODO(mcgrathr): do something with `csrr ..., seed`
  return false;
}

template <bool Reseed>
std::optional<uint64_t> Random<Reseed>::Get(std::optional<unsigned int> retries) {
  ZX_PANIC("arch::Random::Get() called when arch::Random::Supported() fails");
  return std::nullopt;
}

template struct Random<false>;
template struct Random<true>;

}  // namespace arch
