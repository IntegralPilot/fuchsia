// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_CONCURRENT_CHAINLOCK_TRANSACTION_COMMON_H_
#define LIB_CONCURRENT_CHAINLOCK_TRANSACTION_COMMON_H_

#include <lib/concurrent/capability_token.h>
#include <zircon/assert.h>

#include <atomic>
#include <cstddef>
#include <limits>
#include <type_traits>
#include <variant>

namespace concurrent {

constexpr CapabilityToken chainlock_transaction_token;

// ChainLockTransactionCommon is a base class that provides chain lock transaction facilities,
// including transaction token and saved state types, reserved token values, and the global token
// generator. This base class factors out common functionality from ChainLockTransactionBase that is
// not dependent on the runtime environment. ChainLockTransactionBase provides additional
// functionality and simplifies adapting chain locks to the runtime environment.
class ChainLockTransactionCommon {
 public:
  ~ChainLockTransactionCommon() = default;

  ChainLockTransactionCommon(const ChainLockTransactionCommon&) = delete;
  ChainLockTransactionCommon& operator=(const ChainLockTransactionCommon&) = delete;
  ChainLockTransactionCommon(ChainLockTransactionCommon&&) = delete;
  ChainLockTransactionCommon& operator=(ChainLockTransactionCommon&&) = delete;

  static constexpr uint64_t kUnlockedTokenValue{0};
  static constexpr uint64_t kReservedTokenValues{10000};
  static constexpr uint64_t kInitialTokenValue{kReservedTokenValues + 1};
  static constexpr uint64_t kMaxTokenValue{std::numeric_limits<uint64_t>::max()};

  class Token {
   public:
    Token() = default;
    Token(const Token&) = default;
    Token& operator=(const Token& other) = default;

    constexpr bool is_valid() const { return value() != kUnlockedTokenValue; }
    constexpr bool is_reserved() const { return is_valid() && value() <= kReservedTokenValues; }

    constexpr uint64_t value() const { return value_; }

    constexpr bool operator==(const Token& other) const { return value() == other.value(); }
    constexpr bool operator!=(const Token& other) const { return value() != other.value(); }
    constexpr bool operator<(const Token& other) const { return value() < other.value(); }
    constexpr bool operator>(const Token& other) const { return value() > other.value(); }

   private:
    friend ChainLockTransactionCommon;
    template <typename Transaction>
    friend class ChainLock;

    explicit constexpr Token(uint64_t value) : value_{value} {}

    uint64_t value_{kUnlockedTokenValue};
  };

  Token token() const { return token_; }

  class SavedState {
   public:
    SavedState(const SavedState&) = default;
    SavedState& operator=(const SavedState&) = default;

    Token token() const { return token_; }
    ChainLockTransactionCommon* transaction() const { return transaction_; }

   private:
    template <typename Derived, bool DebugAccountingEnabled, bool TracingEnabled>
    friend class ChainLockTransaction;

    explicit SavedState(ChainLockTransactionCommon* transaction)
        : transaction_{transaction}, token_{transaction->token()} {}

    SavedState(ChainLockTransactionCommon* transaction, Token replacement)
        : transaction_{transaction}, token_{transaction->token()} {
      transaction_->ReplaceToken(replacement);
    }

    ChainLockTransactionCommon* transaction_;
    Token token_;
  };

  enum class Action {
    Backoff,
    Restart,
  };

  struct DoneType {};
  static constexpr DoneType Done{};

  template <typename T = DoneType>
  class Result {
   public:
    Result(Action action) : value_{action} {}
    template <typename U, typename = std::enable_if_t<std::is_constructible_v<T, U>>>
    Result(U&& value) : value_{std::forward<U>(value)} {}

    bool is_action() const { return std::holds_alternative<Action>(value_); }

    Action action() const {
      ZX_DEBUG_ASSERT(is_action());
      return std::get<Action>(value_);
    }

    auto take_value() {
      ZX_DEBUG_ASSERT(!is_action());
      using Return = std::conditional_t<std::is_same_v<T, DoneType>, void, T&&>;
      return static_cast<Return>(std::move(std::get<T>(value_)));
    }

   private:
    std::variant<Action, T> value_;
  };

 protected:
  ChainLockTransactionCommon() : token_{NextToken()} {}

  static Token NextToken() {
    return Token{token_generator_.fetch_add(1, std::memory_order_relaxed)};
  }

  static Token ReservedToken(size_t reserved_index) {
    ZX_DEBUG_ASSERT(reserved_index < kReservedTokenValues);
    return Token{reserved_index + 1};
  }

  enum class State {
    Started,
    Finalized,
    Complete,
  };

  State state() const { return state_; }
  void SetState(State state) { state_ = state; }

 private:
  template <typename Derived, bool DebugAccountingEnabled, bool TracingEnabled>
  friend class ChainLockTransaction;

  // Replace a token value when saving or restoring transaction state. Both tokens must be valid and
  // one or the other must be reserved.
  void ReplaceToken(Token replacement) {
    ZX_DEBUG_ASSERT(token_.is_valid());
    ZX_DEBUG_ASSERT(replacement.is_valid());
    ZX_DEBUG_ASSERT(token_.is_reserved() != replacement.is_reserved());
    token_.value_ = replacement.value();
  }

  Token token_;
  State state_{State::Started};
  inline static std::atomic<uint64_t> token_generator_{kInitialTokenValue};
};

}  // namespace concurrent

#endif  // LIB_CONCURRENT_CHAINLOCK_TRANSACTION_COMMON_H_
