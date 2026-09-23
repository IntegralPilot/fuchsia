// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fidl/cpp/wire/status.h>

#include <string>

#include <zxtest/zxtest.h>

namespace {

static_assert(sizeof(fidl::Status) <= sizeof(uint64_t) * 2, "Status should be reasonably small.");
static_assert(std::is_trivially_destructible_v<fidl::Status>,
              "Status should not add destruction overhead");
static_assert(std::is_trivially_copy_constructible_v<fidl::Status>,
              "Status should not add copying overhead");

TEST(Status, ReasonShouldNotBeUsedInOkStatus) {
#ifdef __Fuchsia__
  fidl::Status ok_status = fidl::Status::Ok();
  ASSERT_DEATH(([&] { (void)ok_status.reason(); }), "reason");
#endif  // __Fuchsia__
}

TEST(OneWayStatus, ConvertToFromStatus) {
  // Status -> OneWayStatus is explicit.
  fidl::OneWayStatus one_way_status{fidl::Status::Ok()};
  static_assert(!std::is_convertible_v<fidl::Status, fidl::OneWayStatus>);
  static_assert(std::is_convertible_v<fidl::OneWayStatus, fidl::Status>);
  // OneWayStatus -> Status is implicit.
  fidl::Status status = one_way_status;
  ASSERT_OK(status);
}

// TODO(https://fxbug.dev/42126998): We would be able to remove the differences between
// fuchsia/host if |zx_status_get_string| is available on host.
std::string SelectErrorDescription(const std::string& fuchsia, const std::string& host) {
#ifdef __Fuchsia__
  return fuchsia;
#else
  return host;
#endif  // __Fuchsia__
}

TEST(Status, OkDescription) { ASSERT_EQ("FIDL success", fidl::Status::Ok().FormatDescription()); }

TEST(Status, UnboundDescription) {
  std::string expected = SelectErrorDescription(
      "FIDL operation failed due to user initiated unbind, status: ZX_ERR_CANCELED (-23), "
      "detail: unbound endpoint",
      "FIDL operation failed due to user initiated unbind, status: -23, "
      "detail: unbound endpoint");
  ASSERT_EQ(expected, fidl::Status::Unbound().FormatDescription());
}

TEST(Status, UnknownOrdinalDescription) {
  std::string expected = SelectErrorDescription(
      "FIDL operation failed due to unexpected message, status: ZX_ERR_NOT_SUPPORTED (-2), "
      "detail: unknown ordinal",
      "FIDL operation failed due to unexpected message, status: -2, "
      "detail: unknown ordinal");
  ASSERT_EQ(expected, fidl::Status::UnknownOrdinal().FormatDescription());
}

TEST(Status, TransportErrorDescription) {
  std::string expected = SelectErrorDescription(
      "FIDL operation failed due to underlying transport I/O error, "
      "status: ZX_ERR_INVALID_ARGS (-10), detail: foo",
      "FIDL operation failed due to underlying transport I/O error, "
      "status: -10, detail: foo");
  ASSERT_EQ(expected, fidl::Status::TransportError(ZX_ERR_INVALID_ARGS, "foo").FormatDescription());
}

TEST(Status, PeerClosedDescription) {
  std::string expected = SelectErrorDescription(
      "FIDL operation failed due to peer closed, status: ZX_ERR_PEER_CLOSED (-24)",
      "FIDL operation failed due to peer closed, status: -24");
  ASSERT_EQ(expected, fidl::Status::TransportError(ZX_ERR_PEER_CLOSED).FormatDescription());
}

TEST(Status, EncodeErrorDescription) {
  std::string expected = SelectErrorDescription(
      "FIDL operation failed due to encode error, status: ZX_ERR_INVALID_ARGS (-10), "
      "detail: foo",
      "FIDL operation failed due to encode error, status: -10, detail: foo");
  ASSERT_EQ(expected, fidl::Status::EncodeError(ZX_ERR_INVALID_ARGS, "foo").FormatDescription());
}

TEST(Status, DecodeErrorDescription) {
  std::string expected = SelectErrorDescription(
      "FIDL operation failed due to decode error, "
      "status: ZX_ERR_INVALID_ARGS (-10), detail: foo",
      "FIDL operation failed due to decode error, "
      "status: -10, detail: foo");
  ASSERT_EQ(expected, fidl::Status::DecodeError(ZX_ERR_INVALID_ARGS, "foo").FormatDescription());
}

TEST(Status, UnexpectedMessageDescription) {
  std::string expected = SelectErrorDescription(
      "FIDL operation failed due to unexpected message, "
      "status: ZX_ERR_INVALID_ARGS (-10), detail: foo",
      "FIDL operation failed due to unexpected message, "
      "status: -10, detail: foo");
  ASSERT_EQ(expected,
            fidl::Status::UnexpectedMessage(ZX_ERR_INVALID_ARGS, "foo").FormatDescription());
}

TEST(Status, CanceledDescription) {
  std::string expected = SelectErrorDescription(
      "FIDL operation failed due to being canceled by failure from another operation: "
      "encode error, status: ZX_ERR_CANCELED (-23), detail: string too long",
      "FIDL operation failed due to being canceled by failure from another operation: "
      "encode error, status: -23, detail: string too long");
  auto canceled = fidl::Status::Canceled(
      fidl::UnbindInfo{fidl::Status::EncodeError(ZX_ERR_INVALID_ARGS, "string too long")});
  ASSERT_EQ(expected, canceled.FormatDescription());
}

TEST(Status, Canceled) {
  {
    auto canceled = fidl::Status::Canceled(
        fidl::UnbindInfo{fidl::Status::EncodeError(ZX_ERR_INVALID_ARGS, "string too long")});
    EXPECT_EQ(canceled.reason(), fidl::Reason::kCanceledDueToOtherError);
    EXPECT_EQ(canceled.underlying_reason().value(), fidl::Reason::kEncodeError);
    EXPECT_EQ(canceled.status(), ZX_ERR_CANCELED);
    EXPECT_TRUE(canceled.is_canceled());
  }

  {
    auto regular = fidl::Status::UnknownMethod();
    EXPECT_EQ(regular.reason(), fidl::Reason::kUnknownMethod);
    EXPECT_EQ(regular.underlying_reason(), std::nullopt);
    EXPECT_EQ(regular.status(), ZX_ERR_NOT_SUPPORTED);
    EXPECT_FALSE(regular.is_canceled());
  }
}

TEST(Status, FormatDisplayError) {
  auto r = fidl::Status::Ok();
  char buffer[100];
  fidl::internal::FormatDisplayError(r, buffer, sizeof(buffer));
  ASSERT_EQ("FIDL success", std::string_view(buffer));
}

TEST(UnbindInfo, UnbindDescription) {
  std::string expected = SelectErrorDescription(
      "FIDL endpoint was unbound due to user initiated unbind, status: ZX_OK (0)",
      "FIDL endpoint was unbound due to user initiated unbind, status: 0");
  ASSERT_EQ(expected, fidl::UnbindInfo::Unbind().FormatDescription());
}

TEST(UnbindInfo, CloseDescription) {
  std::string expected = SelectErrorDescription(
      "FIDL endpoint was unbound due to (server) user initiated close with epitaph, "
      "status of sending epitaph: ZX_ERR_INVALID_ARGS (-10)",
      "FIDL endpoint was unbound due to (server) user initiated close with epitaph, "
      "status of sending epitaph: -10");
  ASSERT_EQ(expected, fidl::UnbindInfo::Close(ZX_ERR_INVALID_ARGS).FormatDescription());
}

TEST(UnbindInfo, PeerClosedDescription) {
  std::string expected = SelectErrorDescription(
      "FIDL endpoint was unbound due to peer closed, status: ZX_ERR_PEER_CLOSED (-24)",
      "FIDL endpoint was unbound due to peer closed, status: -24");
  ASSERT_EQ(expected, fidl::UnbindInfo::PeerClosed(ZX_ERR_PEER_CLOSED).FormatDescription());
}

TEST(UnbindInfo, PeerClosedEpitaphDescription) {
  std::string expected = SelectErrorDescription(
      "FIDL endpoint was unbound due to peer closed, epitaph: ZX_ERR_INVALID_ARGS (-10)",
      "FIDL endpoint was unbound due to peer closed, epitaph: -10");
  ASSERT_EQ(expected, fidl::UnbindInfo::PeerClosed(ZX_ERR_INVALID_ARGS).FormatDescription());
}

TEST(UnbindInfo, DispatcherErrorDescription) {
  std::string expected = SelectErrorDescription(
      "FIDL endpoint was unbound due to dispatcher error, "
      "status: ZX_ERR_ACCESS_DENIED (-30)",
      "FIDL endpoint was unbound due to dispatcher error, status: -30");
  ASSERT_EQ(expected, fidl::UnbindInfo::DispatcherError(ZX_ERR_ACCESS_DENIED).FormatDescription());
}

TEST(UnbindInfo, ToError) {
  fidl::Error unbound = fidl::UnbindInfo::Unbind().ToError();
  EXPECT_EQ(unbound.reason(), fidl::Reason::kUnbind);
  EXPECT_EQ(unbound.status(), ZX_ERR_CANCELED);

  fidl::Error peer_closed = fidl::UnbindInfo::PeerClosed(ZX_ERR_PEER_CLOSED).ToError();
  EXPECT_EQ(peer_closed.reason(), fidl::Reason::kPeerClosedWhileReading);
  EXPECT_EQ(peer_closed.status(), ZX_ERR_PEER_CLOSED);
}

}  // namespace
