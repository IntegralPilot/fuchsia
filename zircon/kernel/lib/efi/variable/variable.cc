// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <algorithm>

#include <efi/variable/variable.h>

namespace efi {

VariableValue Copy(const VariableValue& source) {
  VariableValue copy;
  copy.resize(source.size());
  std::copy(source.begin(), source.end(), copy.begin());
  return copy;
}

Variable::Variable(const VariableId& id_in, const VariableValue& value_in)
    : id(id_in), value(Copy(value_in)) {}

Variable::Variable(const Variable& source) : id(source.id), value(Copy(source.value)) {}

Variable& Variable::operator=(const Variable& source) noexcept {
  id = source.id;
  value = Copy(source.value);
  return *this;
}

}  // namespace efi
