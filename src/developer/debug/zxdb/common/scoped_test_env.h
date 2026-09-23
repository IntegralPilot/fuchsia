// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_COMMON_SCOPED_TEST_ENV_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_COMMON_SCOPED_TEST_ENV_H_

#include <cstdlib>
#include <map>
#include <optional>
#include <string>

#include <gtest/gtest.h>

#include "src/lib/fxl/macros.h"

namespace zxdb {

// A helper class that saves original environment variables before modification and restores them on
// destruction. There is no checking of whether or not inputs are valid environment variables and
// callers are responsible for ensuring the return value matches what they expect. The return values
// of |Set| and |Unset| are the same as their corresponding libc counterparts. See man setenv and
// man unsetenv for details.
//
// Edge cases:
//   * Users *may* call |Set| on the same key multiple times, only the original value will be
//     restored.
//   * Calling |Unset| on a key that does not exist in the current environment will return the value
//     of calling |unsetenv| with that same key.
class ScopedTestEnv {
 public:
  ScopedTestEnv() = default;
  ~ScopedTestEnv() {
    for (const auto& [k, v] : restore_) {
      if (!v) {
        unsetenv(k.c_str());
      } else {
        setenv(k.c_str(), v->c_str(), 1);
      }
    }
  }

  int Set(const std::string& key, const std::string& val) {
    RecordOriginal(key);
    return setenv(key.c_str(), val.c_str(), /* replace */ 1);
  }

  int Unset(const std::string& key) {
    RecordOriginal(key);
    // Calling |unsetenv| on a nonexistent key is okay.
    return unsetenv(key.c_str());
  }

 private:
  void RecordOriginal(const std::string& key) {
    const char* old = std::getenv(key.c_str());

    // Only add the key to our mapping if it's the first time it's being modified. Intermediate
    // values of this same key are not restored.
    if (!restore_.contains(key)) {
      if (old) {
        restore_[key] = old;
      } else {
        restore_[key] = std::nullopt;
      }
    }
  }

  FXL_DISALLOW_COPY_ASSIGN_AND_MOVE(ScopedTestEnv);

  // This is currently just a simple 1:1 replacement list that will replace things in alphabetical
  // order with respect to the keys. If we end up needing something more sophisticated than this we
  // can log "transactions" instead and then replay them in reverse to return to the original state.
  std::map<std::string, std::optional<std::string>> restore_;
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_COMMON_SCOPED_TEST_ENV_H_
