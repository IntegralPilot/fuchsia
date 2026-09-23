// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_TESTING_GPRETTY_PRINTERS_H_
#define SRC_DEVELOPER_FORENSICS_TESTING_GPRETTY_PRINTERS_H_

#include <fuchsia/feedback/cpp/fidl.h>
#include <fuchsia/mem/cpp/fidl.h>
#include <lib/fpromise/result.h>
#include <lib/syslog/cpp/macros.h>

#include <ostream>
#include <string_view>

#include <src/lib/fostr/fidl/fuchsia/mem/formatting.h>
#include <src/lib/fostr/indent.h>

#include "src/developer/forensics/crash_reports/filing_result.h"
#include "src/developer/forensics/crash_reports/item_location.h"
#include "src/developer/forensics/crash_reports/product.h"
#include "src/developer/forensics/feedback/attachments/types.h"
#include "src/developer/forensics/feedback/reboot_log/graceful_shutdown_info.h"
#include "src/developer/forensics/feedback/stop_signals.h"
#include "src/developer/forensics/utils/errors.h"
#include "src/lib/fsl/vmo/strings.h"

namespace fit {

// Pretty-prints fpromise::result_state in gTest matchers instead of the default byte string in case
// of failed expectations.
inline void PrintTo(const fpromise::result_state& state, std::ostream* os) {
  std::string state_str;
  switch (state) {
    case fpromise::result_state::pending:
      state_str = "PENDING";
      break;
    case fpromise::result_state::ok:
      state_str = "OK";
      break;
    case fpromise::result_state::error:
      state_str = "ERROR";
      break;
  }
  *os << state_str;
}

}  // namespace fit

namespace forensics {
namespace pretty {

inline std::string ToString(const ErrorOrString& error_or) {
  if (error_or.HasValue()) {
    return error_or.Value();
  }

  return ToString(error_or.Error());
}

}  // namespace pretty

inline void PrintTo(const Error error, std::ostream* os) { *os << ToString(error); }

inline void PrintTo(const ErrorOrString& error_or, std::ostream* os) {
  *os << pretty::ToString(error_or);
}

namespace crash_reports {

// Pretty-prints ItemLocation in gTest matchers instead of the default byte string in case
// of failed expectations.
inline void PrintTo(const ItemLocation& location, std::ostream* os) {
  std::string location_str;
  switch (location) {
    case ItemLocation::kMemory:
      location_str = "MEMORY";
      break;
    case ItemLocation::kCache:
      location_str = "CACHE";
      break;
    case ItemLocation::kTmp:
      location_str = "TMP";
      break;
  }
  *os << location_str;
}

// Pretty-prints FilingResult in gTest matchers instead of the default byte string in case of
// failed expectations.
inline void PrintTo(const FilingResult& result, std::ostream* os) {
  std::string result_str;
  switch (result) {
    case FilingResult::kReportUploaded:
      result_str = "REPORT_UPLOADED";
      break;
    case FilingResult::kReportOnDisk:
      result_str = "REPORT_ON_DISK";
      break;
    case FilingResult::kReportInMemory:
      result_str = "REPORT_IN_MEMORY";
      break;
    case FilingResult::kReportNotFiledUserOptedOut:
      result_str = "REPORT_NOT_FILED_USER_OPTED_OUT";
      break;
    case FilingResult::kInvalidArgsError:
      result_str = "INVALID_ARGS_ERROR";
      break;
    case FilingResult::kServerError:
      result_str = "SERVER_ERROR";
      break;
    case FilingResult::kPersistenceError:
      result_str = "PERSISTENCE_ERROR";
      break;
    case FilingResult::kQuotaReachedError:
      result_str = "QUOTA_REACHED_ERROR";
      break;
  }
  *os << result_str;
}

inline void PrintTo(const Product& product, std::ostream* os) {
  *os << "\nProduct {";
  *os << "\n  name:    " << product.name;
  *os << "\n  version: " << pretty::ToString(product.version);
  *os << "\n  channel: " << pretty::ToString(product.channel);
  *os << "\n}";
}

}  // namespace crash_reports

namespace feedback {

namespace pretty {

// Display ASCII character as is or non-ascii characters by their {hex value}.
inline void Format(const char ch, std::stringstream* output) {
  if (ch == '\n' || ch == '\t') {
    *output << ch;
  } else if (ch >= ' ' && ch <= '~') {
    *output << ch;
  } else {
    *output << "{0x" << ((int)ch & 0xFF) << "}";
  }
}

// Removes exception "FormatException: Unexpected extension byte" when calling PrintTo() with
// ostream = std::cout by converting all non-ascii character to "{hex_value}".
inline std::string Format(const std::string_view input) {
  std::stringstream output;
  // Display HEX code for integer-cast non-ascii characters.
  output << std::hex;
  for (char ch : input) {
    Format(ch, &output);
  }
  return output.str();
}

}  // namespace pretty

inline void PrintTo(const AttachmentData& value, std::ostream* os) {
  *os << fostr::Indent;
  *os << "{ ";
  switch (value.State()) {
    case AttachmentState::kComplete:
      *os << "VALUE : " << pretty::Format(value.Value());
      break;
    case AttachmentState::kPartial:
      *os << "VALUE : " << pretty::Format(value.Value());
      *os << ", ERROR : " << ToString(value.Error());
      break;
    case AttachmentState::kMissing:
      *os << "ERROR : " << ToString(value.Error());
      break;
  }
  *os << " }";
  *os << fostr::Outdent;
}

inline void PrintTo(const AttachmentValue& value, std::ostream* os) {
  *os << fostr::Indent;
  *os << "{ ";
  switch (value.State()) {
    case AttachmentState::kComplete:
      *os << "VALUE : " << pretty::Format(value.Value());
      break;
    case AttachmentState::kPartial:
      *os << "VALUE : " << pretty::Format(value.Value());
      *os << ", ERROR : " << ToString(value.Error());
      break;
    case AttachmentState::kMissing:
      *os << "ERROR : " << ToString(value.Error());
      break;
  }
  *os << " }";
  *os << fostr::Outdent;
}

inline void PrintTo(const GracefulShutdownInfoSignal& signal, std::ostream* os) {
  *os << fostr::Indent;
  *os << fostr::Indent;
  *os << fostr::NewLine << "action: " << ToString(signal.Action());
  *os << fostr::NewLine << "reasons: " << ToRawStrings(signal.Reasons());
  *os << fostr::Outdent;
  *os << fostr::Outdent;
}

inline void PrintTo(const GracefulShutdownAction action, std::ostream* os) {
  *os << ToString(action);
}

}  // namespace feedback

}  // namespace forensics

namespace fuchsia {
namespace feedback {

// Pretty-prints Attachment in gTest matchers instead of the default byte string in case of failed
// expectations.
inline void PrintTo(const Attachment& attachment, std::ostream* os) {
  *os << fostr::Indent;
  *os << fostr::NewLine << "key: " << attachment.key;
  *os << fostr::NewLine << "value: ";
  std::string value;
  if (fsl::StringFromVmo(attachment.value, &value)) {
    if (value.size() < 1024) {
      *os << "'" << value << "'";
    } else {
      *os << "(string too long)" << attachment.value;
    }
  } else {
    *os << attachment.value;
  }
  *os << fostr::Outdent;
}

// Pretty-prints Annotation in gTest matchers instead of the default byte string in case of failed
// expectations.
inline void PrintTo(const Annotation& annotation, std::ostream* os) {
  *os << fostr::Indent;
  *os << fostr::NewLine << "key: " << annotation.key;
  *os << fostr::NewLine << "value: " << annotation.value;
  *os << fostr::Outdent;
}

}  // namespace feedback

namespace mem {

// Pretty-prints string VMOs in gTest matchers instead of the default byte string in case of failed
// expectations.
inline void PrintTo(const Buffer& vmo, std::ostream* os) {
  std::string value;
  FX_CHECK(fsl::StringFromVmo(vmo, &value));
  *os << "'" << value << "'";
}

}  // namespace mem
}  // namespace fuchsia

namespace zx {

inline void PrintTo(const zx::duration& duration, std::ostream* os) {
  *os << duration.to_nsecs() << "ns";
}

}  // namespace zx

#endif  // SRC_DEVELOPER_FORENSICS_TESTING_GPRETTY_PRINTERS_H_
