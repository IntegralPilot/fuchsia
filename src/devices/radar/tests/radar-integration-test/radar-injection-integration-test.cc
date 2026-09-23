// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.radar/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fit/function.h>
#include <lib/zx/time.h>
#include <lib/zx/vmo.h>

#include <map>
#include <memory>
#include <vector>

#include <zxtest/zxtest.h>

namespace {

constexpr uint32_t kBurstSize = 23247;

enum RadarEvent {
  kOnBurst,
  kOnBurstsDelivered,
};

class ReaderClient : public fidl::AsyncEventHandler<fuchsia_hardware_radar::RadarBurstReader> {
 public:
  ReaderClient(fidl::ClientEnd<fuchsia_hardware_radar::RadarBurstReader> client_end,
               async_dispatcher_t* dispatcher, fit::function<void(RadarEvent)> on_test_event)
      : client_(std::move(client_end), dispatcher, this),
        on_test_event_(std::move(on_test_event)) {}

  fidl::Client<fuchsia_hardware_radar::RadarBurstReader>& operator->() { return client_; }

  size_t real_bursts() const { return real_bursts_; }
  size_t injected_bursts() const { return injected_bursts_; }

  void RegisterVmos(uint32_t count) {
    EXPECT_EQ(vmos_.size(), 0);

    std::vector<uint32_t> vmo_ids;
    std::vector<zx::vmo> vmos;

    for (uint32_t i = 0; i < count; i++) {
      vmo_ids.emplace_back(i);
      vmos.emplace_back();
      EXPECT_OK(zx::vmo::create(kBurstSize, 0, &vmos.back()));
      EXPECT_OK(vmos.back().duplicate(ZX_RIGHT_SAME_RIGHTS, &vmos_.emplace_back()));
    }

    client_->RegisterVmos({std::move(vmo_ids), std::move(vmos)}).Then([&](const auto& result) {
      EXPECT_TRUE(result.is_ok());
    });
  }

 private:
  void OnBurst(fidl::Event<fuchsia_hardware_radar::RadarBurstReader::OnBurst>& event) override {
    EXPECT_TRUE(event.burst() || event.error());

    // The radar driver might report real errors -- just ignore them.

    if (event.burst()) {
      const uint32_t vmo_id = event.burst()->vmo_id();
      ASSERT_LE(vmo_id, vmos_.size());

      uint32_t header;
      EXPECT_OK(vmos_[vmo_id].read(&header, 0, sizeof(header)));
      (header == 0 ? real_bursts_ : injected_bursts_)++;

      EXPECT_TRUE(client_->UnlockVmo(vmo_id).is_ok());

      on_test_event_(kOnBurst);
    }
  }

  fidl::Client<fuchsia_hardware_radar::RadarBurstReader> client_;
  const fit::function<void(RadarEvent)> on_test_event_;

  std::vector<zx::vmo> vmos_;
  size_t real_bursts_ = 0;
  size_t injected_bursts_ = 0;
};

class InjectorClient : public fidl::AsyncEventHandler<fuchsia_hardware_radar::RadarBurstInjector> {
 public:
  InjectorClient(fidl::ClientEnd<fuchsia_hardware_radar::RadarBurstInjector> client_end,
                 async_dispatcher_t* dispatcher, fit::function<void(RadarEvent)> on_test_event)
      : client_(std::move(client_end), dispatcher, this),
        on_test_event_(std::move(on_test_event)) {}

  fidl::Client<fuchsia_hardware_radar::RadarBurstInjector>& operator->() { return client_; }

  void EnqueueBursts(uint32_t count) {
    zx::vmo vmo;
    EXPECT_OK(zx::vmo::create(static_cast<size_t>(count) * kBurstSize, 0, &vmo));

    for (uint32_t i = 0; i < count; i++) {
      const size_t offset = static_cast<size_t>(i) * kBurstSize;
      EXPECT_OK(vmo.write(&kInjectedBurstHeader, offset, sizeof(kInjectedBurstHeader)));
    }

    client_->EnqueueBursts({{std::move(vmo), count}}).Then([&](const auto& result) {
      EXPECT_TRUE(result.is_ok());
    });
  }

 private:
  // Use any non-zero value to indicate an injected burst.
  static constexpr uint32_t kInjectedBurstHeader = 0xffff'ffff;

  void OnBurstsDelivered(
      fidl::Event<fuchsia_hardware_radar::RadarBurstInjector::OnBurstsDelivered>& event) override {
    on_test_event_(kOnBurstsDelivered);
  }

  fidl::Client<fuchsia_hardware_radar::RadarBurstInjector> client_;
  const fit::function<void(RadarEvent)> on_test_event_;
};

TEST(RadarInjectionIntegrationTest, InjectBursts) {
  std::optional<ReaderClient> reader1;
  std::optional<ReaderClient> reader2;
  std::optional<InjectorClient> injector;
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);

  enum TestState {
    kReceivedRealBurstsStart,
    kBurstsInjected,
    kStopOneClient,
    kMoreBurstsInjected,
    kReceivedRealBurstsEnd,
    kTestEnd,
  };

  TestState current_state = kReceivedRealBurstsStart;
  size_t burst_count_after_injection = UINT32_MAX;

  const auto on_test_event = [&](const RadarEvent event) {
    TestState next_state = current_state;

    switch (current_state) {
      case kReceivedRealBurstsStart:
        // Both clients have received at least one real burst. Enqueue six bursts to be injected and
        // start injection.
        if (reader1->real_bursts() >= 1 && reader2->real_bursts() >= 1) {
          injector->EnqueueBursts(3);
          injector->EnqueueBursts(3);
          (*injector)->StartBurstInjection().Then(
              [&](const auto& result) { EXPECT_TRUE(result.is_ok()); });
          next_state = kBurstsInjected;
        }
        break;
      case kBurstsInjected:
        // One of our bursts VMOs was just returned. Enqueue another three bursts to be injected.
        if (event == kOnBurstsDelivered) {
          injector->EnqueueBursts(3);
          next_state = kStopOneClient;
        }
        break;
      case kStopOneClient:
        // All nine bursts have been sent to clients. Stop one of the clients, then enqueue three
        // more bursts.
        if (reader2->injected_bursts() >= 9) {
          (*reader2)->StopBursts().Then([&](const auto& result) {
            EXPECT_TRUE(result.is_ok());
            injector->EnqueueBursts(3);
          });
          next_state = kMoreBurstsInjected;
        } else if (event == kOnBurstsDelivered) {
          // Continue injecting bursts until clients receive them all. There's a chance that some
          // will be missed if we don't manage to unlock in time.
          injector->EnqueueBursts(3);
        }
        break;
      case kMoreBurstsInjected:
        // The next three bursts have been sent to clients. Stop burst injection.
        if (reader1->injected_bursts() >= 12) {
          (*injector)->StopBurstInjection().Then([&](const auto& result) {
            burst_count_after_injection = reader1->real_bursts();
            EXPECT_TRUE(result.is_ok());
          });
          next_state = kReceivedRealBurstsEnd;
        } else if (event == kOnBurstsDelivered) {
          injector->EnqueueBursts(3);
        }
        break;
      case kReceivedRealBurstsEnd:
        // At least one more real burst has been received. Stop the client to flush any in-flight
        // bursts.
        if (reader1->real_bursts() > burst_count_after_injection) {
          (*reader1)->StopBursts().Then(
              [&](const fidl::Result<fuchsia_hardware_radar::RadarBurstReader::StopBursts>&
                      result) {
                EXPECT_TRUE(result.is_ok());
                loop.Quit();
              });
          next_state = kTestEnd;
        }
        break;
      case kTestEnd:
        break;
    }

    current_state = next_state;
  };

  auto provider_client_end = component::Connect<fuchsia_hardware_radar::RadarBurstReaderProvider>();
  ASSERT_TRUE(provider_client_end.is_ok());

  fidl::SyncClient<fuchsia_hardware_radar::RadarBurstReaderProvider> provider_client(
      std::move(provider_client_end.value()));

  {
    auto endpoints = fidl::Endpoints<fuchsia_hardware_radar::RadarBurstReader>::Create();

    const auto result = provider_client->Connect(std::move(endpoints.server));
    EXPECT_TRUE(result.is_ok());

    reader1.emplace(std::move(endpoints.client), loop.dispatcher(), on_test_event);
    EXPECT_NO_FAILURES(reader1->RegisterVmos(10));
  }

  {
    auto endpoints = fidl::Endpoints<fuchsia_hardware_radar::RadarBurstReader>::Create();

    const auto result = provider_client->Connect(std::move(endpoints.server));
    EXPECT_TRUE(result.is_ok());

    reader2.emplace(std::move(endpoints.client), loop.dispatcher(), on_test_event);
    EXPECT_NO_FAILURES(reader2->RegisterVmos(10));
  }

  auto injector_client_end = component::Connect<fuchsia_hardware_radar::RadarBurstInjector>();
  ASSERT_TRUE(injector_client_end.is_ok());

  injector.emplace(std::move(injector_client_end.value()), loop.dispatcher(), on_test_event);

  EXPECT_TRUE((*reader1)->StartBursts().is_ok());
  EXPECT_TRUE((*reader2)->StartBursts().is_ok());

  loop.Run();
}

TEST(RadarInjectionIntegrationTest, BurstsResumedAfterInjectorDisconnects) {
  std::optional<ReaderClient> reader;
  std::optional<InjectorClient> injector;
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);

  enum TestState {
    kReceivedRealBurstsStart,
    kDisconnectInjector,
    kReceivedRealBurstsEnd,
    kTestEnd,
  };

  TestState current_state = kReceivedRealBurstsStart;
  size_t burst_count_after_injection = UINT32_MAX;

  const auto on_test_event = [&](const RadarEvent event) {
    TestState next_state = current_state;

    switch (current_state) {
      case kReceivedRealBurstsStart:
        // The client has received at least one real burst. Enqueue three bursts to be injected and
        // start injection.
        if (reader->real_bursts() >= 1) {
          injector->EnqueueBursts(3);
          (*injector)->StartBurstInjection().Then(
              [&](const auto& result) { EXPECT_TRUE(result.is_ok()); });
          next_state = kDisconnectInjector;
        }
        break;
      case kDisconnectInjector:
        // The client has received all three injected bursts. Disconnect the injector and wait for
        // radar-proxy to restart bursts from the driver.
        if (reader->injected_bursts() >= 3) {
          injector.reset();
          burst_count_after_injection = reader->real_bursts();
          next_state = kReceivedRealBurstsEnd;
        } else if (event == kOnBurstsDelivered) {
          injector->EnqueueBursts(3);
        }
        break;
      case kReceivedRealBurstsEnd:
        // The client has received additional bursts from the driver.
        if (reader->real_bursts() > burst_count_after_injection) {
          (*reader)->StopBursts().Then([&](const auto& result) {
            EXPECT_TRUE(result.is_ok());
            loop.Quit();
          });
          next_state = kTestEnd;
        }
        break;
      case kTestEnd:
        break;
    }

    current_state = next_state;
  };

  {
    auto provider_client_end =
        component::Connect<fuchsia_hardware_radar::RadarBurstReaderProvider>();
    ASSERT_TRUE(provider_client_end.is_ok());

    fidl::SyncClient<fuchsia_hardware_radar::RadarBurstReaderProvider> provider_client(
        std::move(provider_client_end.value()));

    auto endpoints = fidl::Endpoints<fuchsia_hardware_radar::RadarBurstReader>::Create();

    const auto result = provider_client->Connect(std::move(endpoints.server));
    EXPECT_TRUE(result.is_ok());

    reader.emplace(std::move(endpoints.client), loop.dispatcher(), on_test_event);
    EXPECT_NO_FAILURES(reader->RegisterVmos(10));
  }

  {
    auto injector_client_end = component::Connect<fuchsia_hardware_radar::RadarBurstInjector>();
    ASSERT_TRUE(injector_client_end.is_ok());

    injector.emplace(std::move(injector_client_end.value()), loop.dispatcher(), on_test_event);
  }

  EXPECT_TRUE((*reader)->StartBursts().is_ok());

  loop.Run();
}

TEST(RadarInjectionIntegrationTest, OnlyOneInjectorCanConnect) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);

  auto injector_client_end = component::Connect<fuchsia_hardware_radar::RadarBurstInjector>();
  ASSERT_TRUE(injector_client_end.is_ok());

  fidl::Client<fuchsia_hardware_radar::RadarBurstInjector> injector(
      std::move(injector_client_end.value()), loop.dispatcher());

  injector->GetBurstProperties().Then([&](const auto& result) {
    EXPECT_TRUE(result.is_ok());

    auto injector_new_client_end = component::Connect<fuchsia_hardware_radar::RadarBurstInjector>();
    ASSERT_TRUE(injector_new_client_end.is_ok());

    fidl::SyncClient<fuchsia_hardware_radar::RadarBurstInjector> injector_new(
        std::move(injector_new_client_end.value()));

    EXPECT_FALSE(injector_new->GetBurstProperties().is_ok());
    loop.Quit();
  });

  loop.Run();
}

}  // namespace
