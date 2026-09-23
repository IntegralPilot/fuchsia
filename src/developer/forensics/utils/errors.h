// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_UTILS_ERRORS_H_
#define SRC_DEVELOPER_FORENSICS_UTILS_ERRORS_H_

#include <lib/fidl/cpp/wire/status.h>
#include <lib/fpromise/promise.h>
#include <lib/syslog/cpp/macros.h>

#include <string>
#include <variant>

#include "lib/utf-utils/utf-utils.h"

namespace forensics {

// Defines common errors that occur throughout //src/developer/feedback.
enum class Error {
  kNotSet,
  // TODO(https://fxbug.dev/42126944): Remove kDefault. This value is temporary to allow the enum to
  // be used without specifying the exact error that occurred.
  kDefault,
  kLogicError,
  kTimeout,
  kConnectionError,
  kAsyncTaskPostFailure,
  kMissingValue,
  kBadValue,
  kInvalidFormat,
  kFileReadFailure,
  kFileWriteFailure,
  kNotAvailableInProduct,
  // Custom errors code that can be interpreted in different ways by different components.
  kCustom,
};

Error FidlErrorToForensicsError(const ::fidl::Error& error);

class ErrorOrString {
 public:
  explicit ErrorOrString(std::string value) : data_(GetValueIfValidUtf8(std::move(value))) {}
  explicit ErrorOrString(enum Error error) : data_(error) {}

  // Allow construction from a ::fpromise::result.
  explicit ErrorOrString(::fpromise::result<std::string, Error> result) {
    if (result.is_ok()) {
      data_ = GetValueIfValidUtf8(std::move(result.value()));
    } else {
      data_ = result.error();
    }
  }

  bool HasValue() const { return data_.index() == 0; }

  const std::string& Value() const {
    FX_CHECK(HasValue());
    return std::get<std::string>(data_);
  }

  enum Error Error() const {
    FX_CHECK(!HasValue());
    return std::get<enum Error>(data_);
  }

  bool operator==(const ErrorOrString& other) const { return data_ == other.data_; }
  bool operator!=(const ErrorOrString& other) const { return !(*this == other); }
  bool operator==(const enum Error error) const { return !HasValue() && (Error() == error); }
  bool operator!=(const enum Error error) const { return !(*this == error); }

 private:
  static std::variant<std::string, enum Error> GetValueIfValidUtf8(std::string value) {
    if (utfutils_is_valid_utf8(value.c_str(), value.size())) {
      return value;
    }

    return Error::kInvalidFormat;
  }

  std::variant<std::string, enum Error> data_;
};

// Provide a string representation of  |error|.
inline std::string ToString(Error error) {
  switch (error) {
    case Error::kNotSet:
      return "Error::kNotSet";
    case Error::kDefault:
      return "Error::kDefault";
    case Error::kLogicError:
      return "Error::kLogicError";
    case Error::kTimeout:
      return "Error::kTimeout";
    case Error::kConnectionError:
      return "Error::kConnectionError";
    case Error::kAsyncTaskPostFailure:
      return "Error::kAsyncTaskPostFailure";
    case Error::kMissingValue:
      return "Error::kMissingValue";
    case Error::kBadValue:
      return "Error::kBadValue";
    case Error::kInvalidFormat:
      return "Error::kInvalidFormat";
    case Error::kFileReadFailure:
      return "Error::kFileReadFailure";
    case Error::kFileWriteFailure:
      return "Error::kFileWriteFailure";
    case Error::kNotAvailableInProduct:
      return "Error::kNotAvailableInProduct";
    case Error::kCustom:
      return "Error::kCustom";
  }
}

}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_UTILS_ERRORS_H_
