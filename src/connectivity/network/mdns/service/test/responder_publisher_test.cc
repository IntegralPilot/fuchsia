// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <queue>

#include "src/connectivity/network/mdns/service/services/mdns_deprecated_service_impl.h"
#include "src/lib/testing/loop_fixture/test_loop_fixture.h"

namespace mdns {
namespace test {

class ResponderPublisherTest : public gtest::TestLoopFixture,
                               public fuchsia::net::mdns::PublicationResponder {
 public:
  struct OnPublicationCall {
    OnPublicationCall(fuchsia::net::mdns::PublicationCause publication_cause,
                      fidl::StringPtr subtype,
                      std::vector<fuchsia::net::IpAddress> source_addresses,
                      OnPublicationCallback callback)
        : publication_cause_(publication_cause),
          subtype_(subtype),
          source_addresses_(std::move(source_addresses)),
          callback_(std::move(callback)) {}

    fuchsia::net::mdns::PublicationCause publication_cause_;
    fidl::StringPtr subtype_;
    std::vector<fuchsia::net::IpAddress> source_addresses_;
    OnPublicationCallback callback_;
  };

  ResponderPublisherTest() : binding_(this) {}

  ~ResponderPublisherTest() override = default;

  void Bind(fidl::InterfaceRequest<fuchsia::net::mdns::PublicationResponder> request) {
    binding_.Bind(std::move(request));
  }

  void Unbind() { binding_.Unbind(); }

  std::queue<OnPublicationCall>& on_publication_calls() { return on_publication_calls_; }

  void set_on_publication_handler(fit::closure handler) {
    on_publication_handler_ = std::move(handler);
  }

  // fuchsia::net::mdns::PublicationResponder implementation.
  void OnPublication(fuchsia::net::mdns::PublicationCause publication_cause,
                     fidl::StringPtr subtype, std::vector<fuchsia::net::IpAddress> source_addresses,
                     OnPublicationCallback callback) override {
    on_publication_calls_.emplace(publication_cause, subtype, std::move(source_addresses),
                                  std::move(callback));
    if (on_publication_handler_) {
      on_publication_handler_();
    }
  }

 private:
  fidl::Binding<fuchsia::net::mdns::PublicationResponder> binding_;
  std::queue<OnPublicationCall> on_publication_calls_;
  fit::closure on_publication_handler_;
};

// Tests that flow control of |OnPublication| calls works properly.
TEST_F(ResponderPublisherTest, FlowControl) {
  fuchsia::net::mdns::PublicationResponderPtr responder;
  Bind(responder.NewRequest());

  bool deleter_called = false;
  MdnsDeprecatedServiceImpl::ResponderPublisher under_test(
      std::move(responder),
      [](fuchsia::net::mdns::Publisher_PublishServiceInstance_Result result) {},
      [&deleter_called]() { deleter_called = true; });

  // Ask the publisher for a publication and expect that the request is forwarded over FIDL.
  under_test.GetPublication(PublicationCause::kAnnouncement, "1", {},
                            [](std::unique_ptr<Mdns::Publication> publication) {});
  RunLoopUntilIdle();
  EXPECT_EQ(1u, on_publication_calls().size());

  // Ask the publisher for a second publication and expect that this one is also forwarded.
  under_test.GetPublication(PublicationCause::kAnnouncement, "2", {},
                            [](std::unique_ptr<Mdns::Publication> publication) {});
  RunLoopUntilIdle();
  EXPECT_EQ(2u, on_publication_calls().size());

  // Ask the publisher for a third publication. Expect that it's not forwarded yet, because we
  // haven't responded to either of the first two.
  under_test.GetPublication(PublicationCause::kAnnouncement, "3", {},
                            [](std::unique_ptr<Mdns::Publication> publication) {});
  RunLoopUntilIdle();
  EXPECT_EQ(2u, on_publication_calls().size());

  // Respond to the first request and expect the third request to be forwarded.
  EXPECT_EQ("1", on_publication_calls().front().subtype_);
  on_publication_calls().front().callback_(nullptr);
  on_publication_calls().pop();
  EXPECT_EQ(1u, on_publication_calls().size());
  RunLoopUntilIdle();
  EXPECT_EQ(2u, on_publication_calls().size());

  // Ask the publisher for a fourth publication. Expect that it's not forwarded yet, because we
  // haven't responded to either of the second and third requests.
  under_test.GetPublication(PublicationCause::kAnnouncement, "4", {},
                            [](std::unique_ptr<Mdns::Publication> publication) {});
  RunLoopUntilIdle();
  EXPECT_EQ(2u, on_publication_calls().size());

  // Respond to the second request and expect the fourth request to be forwarded.
  EXPECT_EQ("2", on_publication_calls().front().subtype_);
  on_publication_calls().front().callback_(nullptr);
  on_publication_calls().pop();
  EXPECT_EQ(1u, on_publication_calls().size());
  RunLoopUntilIdle();
  EXPECT_EQ(2u, on_publication_calls().size());

  // Respond to the third and fourth requests.
  EXPECT_EQ("3", on_publication_calls().front().subtype_);
  on_publication_calls().front().callback_(nullptr);
  on_publication_calls().pop();
  EXPECT_EQ("4", on_publication_calls().front().subtype_);
  on_publication_calls().front().callback_(nullptr);
  on_publication_calls().pop();
  RunLoopUntilIdle();
  EXPECT_EQ(0u, on_publication_calls().size());

  EXPECT_FALSE(deleter_called);
}

// Tests that closing the responder connection during GetPublication doesn't delete
// ResponderPublisher until GetPublicationNow completes.
TEST_F(ResponderPublisherTest, DisconnectionDuringGetPublication) {
  fuchsia::net::mdns::PublicationResponderPtr responder;
  Bind(responder.NewRequest());

  bool deleter_called = false;
  std::unique_ptr<MdnsDeprecatedServiceImpl::ResponderPublisher> under_test;
  under_test = std::make_unique<MdnsDeprecatedServiceImpl::ResponderPublisher>(
      std::move(responder),
      [](fuchsia::net::mdns::Publisher_PublishServiceInstance_Result result) {},
      [&deleter_called, &under_test]() {
        deleter_called = true;
        under_test.reset();
      });

  // Close the responder binding when OnPublication is invoked.
  set_on_publication_handler([this]() { Unbind(); });

  under_test->GetPublication(PublicationCause::kAnnouncement, "1", {},
                             [](std::unique_ptr<Mdns::Publication> publication) {});

  RunLoopUntilIdle();
  EXPECT_TRUE(deleter_called);
  EXPECT_EQ(nullptr, under_test);
}

// Tests that queries with invalid UTF-8 subtypes are ignored.
TEST_F(ResponderPublisherTest, InvalidSubtypeUtf8) {
  fuchsia::net::mdns::PublicationResponderPtr responder;
  Bind(responder.NewRequest());

  bool deleter_called = false;
  MdnsDeprecatedServiceImpl::ResponderPublisher under_test(
      std::move(responder),
      [](fuchsia::net::mdns::Publisher_PublishServiceInstance_Result result) {},
      [&deleter_called]() { deleter_called = true; });

  bool callback_called = false;
  under_test.GetPublication(PublicationCause::kQueryMulticastResponse, "\xff", {},
                            [&callback_called](std::unique_ptr<Mdns::Publication> publication) {
                              callback_called = true;
                              EXPECT_EQ(nullptr, publication);
                            });

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_called);
  EXPECT_EQ(0u, on_publication_calls().size());
  EXPECT_FALSE(deleter_called);
}

}  // namespace test
}  // namespace mdns
