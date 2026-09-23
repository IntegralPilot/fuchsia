// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_DIAGNOSTICS_ACCESSOR2LOGGER_LOG_MESSAGE_H_
#define SRC_LIB_DIAGNOSTICS_ACCESSOR2LOGGER_LOG_MESSAGE_H_

#include <fuchsia/diagnostics/cpp/fidl.h>
#include <fuchsia/logger/cpp/fidl.h>
#include <lib/fpromise/result.h>
#include <lib/syslog/cpp/log_level.h>

#include <span>
#include <vector>

#include <src/lib/diagnostics/log/message/rust/cpp-log-decoder/log_decoder.h>

namespace diagnostics::accessor2logger {

// Custom deleter for MessageParser. MessageParser is allocated across the Rust FFI
// boundary via `fuchsia_new_message_parser()` and must be freed using
// `fuchsia_free_message_parser()`. Wrapping it in a std::unique_ptr with this deleter
// ensures safe RAII cleanup and correct move semantics (resetting moved-from pointers).
struct MessageParserDeleter {
  void operator()(MessageParser* parser) const;
};

// LogBatchIterator wraps a fuchsia::diagnostics::BatchIterator and maintains the necessary state
// (MessageParser) to decode log messages from it.
// This class is thread-hostile; the same restrictions as InterfacePtr
// (see the comment for InterfacePtr at sdk/lib/fidl/hlcpp/include/lib/fidl/cpp/interface_ptr.h)
// apply to this class. In short, all calls to this class (including the destructor) should
// be made from the thread to which the interface was bound.
class LogBatchIterator {
 public:
  explicit LogBatchIterator(fuchsia::diagnostics::BatchIteratorPtr iterator,
                            fuchsia::diagnostics::Format format);
  ~LogBatchIterator() = default;

  // Non-copyable, but movable.
  LogBatchIterator(const LogBatchIterator&) = delete;
  LogBatchIterator& operator=(const LogBatchIterator&) = delete;
  LogBatchIterator(LogBatchIterator&&) = default;
  LogBatchIterator& operator=(LogBatchIterator&&) = default;

  // Callback type for GetNext. The argument is a result:
  // - The outer fpromise::result indicates the success of fetching and
  //   partially processing the batch. An Err(std::string) here means a
  //   failure in the overall GetNext operation (e.g., FIDL error, or an error
  //   processing the entire payload from the BatchIterator).
  // - If the outer result is Ok, it contains a vector. Each element in the
  //   vector is an fpromise::result representing an individual log message:
  //   - An Ok(fuchsia::logger::LogMessage) means a single log message was
  //     successfully decoded from the batch.
  //   - An Err(std::string) means that a specific item in the batch could not
  //     be converted into a fuchsia::logger::LogMessage. This could be due to
  //     malformed data for that specific log entry.
  using GetNextCallback = fit::callback<void(
      fpromise::result<std::vector<fpromise::result<fuchsia::logger::LogMessage, std::string>>,
                       std::string>)>;

  // Calls GetNext on the underlying BatchIterator and converts the results.
  //
  // This is an asynchronous operation. The method returns immediately, and the
  // provided `callback` will be invoked later when a batch of messages is retrieved
  // or an error occurs.
  //
  // Callback details:
  // - Threading: The callback is invoked on the thread running the dispatcher
  //   associated with the underlying FIDL client (`iterator_`), which is typically
  //   the default dispatcher of the thread where the `LogBatchIterator` was created.
  // - Execution: Since `GetNextCallback` is a `fit::callback`, it will be invoked
  //   at most once.
  // - Lifetime/Cancellation: If the `LogBatchIterator` is destroyed or unbound
  //   (via `Unbind()`) before the callback is run, the pending FIDL callback
  //   will be discarded and the callback will *never* be called. Therefore,
  //   any references or captures within the callback must remain valid until either the
  //   callback is invoked or the `LogBatchIterator` is destroyed/unbound.
  void GetNext(GetNextCallback callback);

  void Unbind() { iterator_.Unbind(); }

  void set_error_handler(fit::function<void(zx_status_t)> handler) {
    iterator_.set_error_handler(std::move(handler));
  }

 private:
  fuchsia::diagnostics::BatchIteratorPtr iterator_;
  std::unique_ptr<MessageParser, MessageParserDeleter> parser_;
};

// Prints formatted content to the log.
fpromise::result<std::vector<fpromise::result<fuchsia::logger::LogMessage, std::string>>,
                 std::string>
ConvertFormattedContentToLogMessages(fuchsia::diagnostics::FormattedContent content);

// Get the severity corresponding to the given verbosity. Note that
// verbosity relative to the default severity and can be thought of
// as incrementally "more vebose than" the baseline.
fuchsia_logging::RawLogSeverity GetSeverityFromVerbosity(uint8_t verbosity);

fpromise::result<std::vector<fpromise::result<fuchsia::logger::LogMessage, std::string>>,
                 std::string>
ConvertFormattedFxtToLogMessages(std::span<const uint8_t> data, bool expect_extended_attribution);

}  // namespace diagnostics::accessor2logger

#endif  // SRC_LIB_DIAGNOSTICS_ACCESSOR2LOGGER_LOG_MESSAGE_H_
