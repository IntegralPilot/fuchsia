// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../radar-reader-proxy.h"

#include <fidl/fuchsia.hardware.radar/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>

#include <functional>
#include <optional>
#include <vector>

#include <zxtest/zxtest.h>

#include "radar-proxy-test.h"

namespace radar {
namespace {

using RadarReaderProxyTest = RadarProxyTest<RadarReaderProxy>;

// TODO(465095319): Fix the flakes and re-enable these test cases.
TEST_F(RadarReaderProxyTest, DISABLED_Connect) {
  AddRadarDevice();

  zx::result endpoints = fidl::CreateEndpoints<fuchsia_hardware_radar::RadarBurstReader>();
  ASSERT_TRUE(endpoints.is_ok());

  dut_client_->Connect(std::move(endpoints->server)).Then([&](auto& result) {
    EXPECT_TRUE(result.is_ok());
  });

  fidl::Client radar_client(std::move(endpoints->client), loop().dispatcher());
  radar_client->GetBurstProperties().Then([&](auto& result) {
    ASSERT_TRUE(result.is_ok());
    EXPECT_EQ(result->size(), 12345);
    loop().Quit();
  });
  loop().Run();
}

TEST_F(RadarReaderProxyTest, DISABLED_Reconnect) {
  AddRadarDevice();

  zx::result endpoints = fidl::CreateEndpoints<fuchsia_hardware_radar::RadarBurstReader>();
  ASSERT_TRUE(endpoints.is_ok());

  dut_client_->Connect(std::move(endpoints->server)).Then([&](auto& result) {
    EXPECT_TRUE(result.is_ok());
  });

  fidl::Client radar_client(std::move(endpoints->client), loop().dispatcher());
  radar_client->GetBurstProperties().Then([&](auto& result) {
    ASSERT_TRUE(result.is_ok());
    EXPECT_EQ(result->size(), 12345);
    loop().Quit();
  });
  loop().Run();
  loop().ResetQuit();

  // Close the Provider connection between the proxy and the radar driver. Everything should still
  // work as long as the Reader connection remains open.
  UnbindRadarDevice();

  // Verify that Connect() still works.
  endpoints = fidl::CreateEndpoints<fuchsia_hardware_radar::RadarBurstReader>();
  ASSERT_TRUE(endpoints.is_ok());

  dut_client_->Connect(std::move(endpoints->server)).Then([&](auto& result) {
    EXPECT_TRUE(result.is_ok());
  });

  fidl::Client new_radar_client(std::move(endpoints->client), loop().dispatcher());
  new_radar_client->GetBurstProperties().Then([&](auto& result) {
    ASSERT_TRUE(result.is_ok());
    EXPECT_EQ(result->size(), 12345);
    loop().Quit();
  });
  loop().Run();
  loop().ResetQuit();

  // Verify that the original connection still works.
  radar_client->GetBurstProperties().Then([&](auto& result) {
    ASSERT_TRUE(result.is_ok());
    EXPECT_EQ(result->size(), 12345);
    loop().Quit();
  });
  loop().Run();
  loop().ResetQuit();

  UnbindReaderAndWaitForConnectionAttempt();

  // Client connections were closed, new requests should now fail.
  radar_client->GetBurstProperties().Then([&](auto& result) {
    ASSERT_FALSE(result.is_ok());
    loop().Quit();
  });
  loop().Run();
  loop().ResetQuit();

  endpoints = fidl::CreateEndpoints<fuchsia_hardware_radar::RadarBurstReader>();
  ASSERT_TRUE(endpoints.is_ok());

  new_radar_client = fidl::Client(std::move(endpoints->client), loop().dispatcher());

  // Send a connect request while there is no radar driver. It should be completed (and the
  // connection usable) after a new radar driver is added.
  dut_client_->Connect(std::move(endpoints->server)).Then([&](auto& result) {
    EXPECT_TRUE(result.is_ok());
  });

  // Signal that a new device was added to /dev/class/radar.
  AddRadarDevice();

  // Verify that the previous connect request suceeded.
  new_radar_client->GetBurstProperties().Then([&](auto& result) {
    ASSERT_TRUE(result.is_ok());
    EXPECT_EQ(result->size(), 12345);
    loop().Quit();
  });
  loop().Run();
  loop().ResetQuit();

  // Subsequent connect requests should be immediately fulfilled.
  endpoints = fidl::CreateEndpoints<fuchsia_hardware_radar::RadarBurstReader>();
  ASSERT_TRUE(endpoints.is_ok());

  dut_client_->Connect(std::move(endpoints->server)).Then([&](auto& result) {
    EXPECT_TRUE(result.is_ok());
  });

  new_radar_client = fidl::Client(std::move(endpoints->client), loop().dispatcher());
  new_radar_client->GetBurstProperties().Then([&](auto& result) {
    ASSERT_TRUE(result.is_ok());
    EXPECT_EQ(result->size(), 12345);
    loop().Quit();
  });
  loop().Run();
  loop().ResetQuit();
}

TEST_F(RadarReaderProxyTest, DISABLED_DelayedConnect) {
  zx::result endpoints = fidl::CreateEndpoints<fuchsia_hardware_radar::RadarBurstReader>();
  ASSERT_TRUE(endpoints.is_ok());

  // Attempt to connect before a radar device is available. Our request should be put in a queue and
  // completed when a device is added.
  dut_client_->Connect(std::move(endpoints->server)).Then([&](auto& result) {
    EXPECT_TRUE(result.is_ok());
  });

  fidl::Client radar_client(std::move(endpoints->client), loop().dispatcher());

  uint32_t burst_size = 0;
  radar_client->GetBurstProperties().Then([&](auto& result) {
    burst_size = result.is_ok() ? result->size() : 0;
    loop().Quit();
  });

  loop().RunUntilIdle();

  // Signal to the proxy that a device was added. This should cause it to process any connect
  // requests that came in before this point.
  AddRadarDevice();

  // The loop will be stopped after the burst size call completes.
  loop().Run();

  EXPECT_EQ(burst_size, 12345);
}

TEST_F(RadarReaderProxyTest, DISABLED_RegisterVmosError) {
  AddRadarDevice();

  struct : fidl::AsyncEventHandler<fuchsia_hardware_radar::RadarBurstReader> {
    void on_fidl_error(fidl::UnbindInfo info) override { loop->Quit(); }
    async::Loop* loop;
  } radar_client;
  radar_client.loop = &loop();

  zx::result endpoints = fidl::CreateEndpoints<fuchsia_hardware_radar::RadarBurstReader>();
  ASSERT_TRUE(endpoints.is_ok());

  dut_client_->Connect(std::move(endpoints->server)).Then([&](auto& result) {
    EXPECT_TRUE(result.is_ok());
  });

  fidl::Client<fuchsia_hardware_radar::RadarBurstReader> client(std::move(endpoints->client),
                                                                loop().dispatcher(), &radar_client);

  {
    std::vector<zx::vmo> vmos;
    EXPECT_OK(zx::vmo::create(12345, 0, &vmos.emplace_back()));
    client->RegisterVmos({std::vector<uint32_t>{1}, std::move(vmos)}).Then([&](auto& result) {
      EXPECT_TRUE(result.is_ok());
    });
  }

  {
    std::vector<zx::vmo> vmos;
    EXPECT_OK(zx::vmo::create(12345, 0, &vmos.emplace_back()));
    // The call to radar-proxy should succeed, however the call from radar-proxy to the underlying
    // driver should fail. When that happens radar-proxy should send an epitaph to the client.
    client->RegisterVmos({std::vector<uint32_t>{2}, std::move(vmos)}).Then([&](auto& result) {
      EXPECT_TRUE(result.is_ok());
    });
  }

  // Run until on_fidl_error receives the epitaph and stops us.
  loop().Run();
}

TEST_F(RadarReaderProxyTest, DISABLED_DeviceWithInvalidBurstSizeIsRejected) {
  AddRadarDevice();

  zx::result endpoints = fidl::CreateEndpoints<fuchsia_hardware_radar::RadarBurstReader>();
  ASSERT_TRUE(endpoints.is_ok());

  fidl::Client<fuchsia_hardware_radar::RadarBurstReader> radar_client(std::move(endpoints->client),
                                                                      loop().dispatcher());

  dut_client_->Connect(std::move(endpoints->server)).Then([&](auto& result) {
    EXPECT_TRUE(result.is_ok());
    radar_client->GetBurstProperties().Then([&](auto& result) {
      ASSERT_TRUE(result.is_ok());
      EXPECT_EQ(result->size(), 12345);

      UnbindRadarDevice();
      UnbindReaderAndWaitForConnectionAttempt();

      zx::result endpoints = fidl::CreateEndpoints<fuchsia_hardware_radar::RadarBurstReader>();
      ASSERT_TRUE(endpoints.is_ok());

      radar_client = fidl::Client(std::move(endpoints->client), loop().dispatcher());

      // With no connected device this request should be put in the queue and completed later.
      dut_client_->Connect(std::move(endpoints->server)).Then([&](auto& result) {
        EXPECT_TRUE(result.is_ok());
        radar_client->GetBurstProperties().Then(
            [&](fidl::Result<fuchsia_hardware_radar::RadarBurstReader::GetBurstProperties>&
                    result) {
              ASSERT_TRUE(result.is_ok());
              EXPECT_EQ(result->size(), 12345);
              loop().Quit();
            });
      });

      // The burst size reported by this device does not match the original value, so it should
      // be ignored.
      fake_driver_.set_burst_size(12346);
      AddRadarDevice();

      // This device matches the original, so it should be accepted and used to complete the
      // connect request.
      fake_driver_.set_burst_size(12345);
      AddRadarDevice();
    });
  });

  loop().Run();
}

// A client wrapper that supports receiving OnBurst() events. If no burst callback is specified then
// VMOs will not be unlocked.
struct BurstReaderClient : fidl::AsyncEventHandler<fuchsia_hardware_radar::RadarBurstReader> {
 public:
  BurstReaderClient(fidl::ClientEnd<fuchsia_hardware_radar::RadarBurstReader> client_end,
                    async_dispatcher_t* dispatcher, std::function<void(uint32_t)> callback)
      : client(std::move(client_end), dispatcher, this), burst_callback(std::move(callback)) {}

  void OnBurst(fidl::Event<fuchsia_hardware_radar::RadarBurstReader::OnBurst>& event) override {
    if (event.burst()) {
      burst_count++;
      if (burst_callback) {
        burst_callback(event.burst()->vmo_id());
        EXPECT_TRUE(client->UnlockVmo(event.burst()->vmo_id()).is_ok());
      }
    } else {
      error_status = event.error().value();
    }
  }

  fidl::Client<fuchsia_hardware_radar::RadarBurstReader>& operator->() { return client; }

  fidl::Client<fuchsia_hardware_radar::RadarBurstReader> client;
  uint32_t burst_count = 0;
  fuchsia_hardware_radar::StatusCode error_status = fuchsia_hardware_radar::StatusCode::kSuccess;

 private:
  const std::function<void(uint32_t)> burst_callback;
};

class RadarReaderProxyOnBurstTest : public RadarReaderProxyTest {
 public:
  RadarReaderProxyOnBurstTest() {}

  void SetUp() override {
    RadarReaderProxyTest::SetUp();

    AddRadarDevice();

    const std::function burst_callback([&](uint32_t) { BurstCallback(); });

    radar_clients_.reserve(4);
    for (uint32_t i = 0; i < 4; i++) {
      zx::result endpoints = fidl::CreateEndpoints<fuchsia_hardware_radar::RadarBurstReader>();
      ASSERT_TRUE(endpoints.is_ok());

      dut_client_->Connect(std::move(endpoints->server)).Then([&](auto& result) {
        EXPECT_TRUE(result.is_ok());
        loop().Quit();
      });
      loop().Run();
      loop().ResetQuit();

      radar_clients_.emplace_back(std::move(endpoints->client), loop().dispatcher(),
                                  // Make the last client not unlock VMOs to test error cases.
                                  i == 3 ? std::function<void(uint32_t)>{} : burst_callback);

      std::vector<zx::vmo> vmos;
      EXPECT_OK(zx::vmo::create(12345, 0, &vmos.emplace_back()));
      EXPECT_OK(zx::vmo::create(12345, 0, &vmos.emplace_back()));
      EXPECT_OK(zx::vmo::create(12345, 0, &vmos.emplace_back()));
      EXPECT_OK(zx::vmo::create(12345, 0, &vmos.emplace_back()));
      EXPECT_OK(zx::vmo::create(12345, 0, &vmos.emplace_back()));
      radar_clients_.back()
          ->RegisterVmos({std::vector<uint32_t>{1, 2, 3, 4, 5}, std::move(vmos)})
          .Then([&](auto& result) { EXPECT_TRUE(result.is_ok()); });
    }
  }

 protected:
  std::vector<BurstReaderClient> radar_clients_;

 private:
  // Assume that there are always this many connected clients with bursts started. A counter will be
  // compared to this value to ensure that the driver doesn't send a new burst until all clients
  // have unlocked their VMOs.
  static constexpr uint32_t kClientsWithBurstsStarted = 2;
  // Keep running until all clients receive this many bursts, then quit the loop.
  static constexpr uint32_t kReceiveBurstCount = 10;

  void BurstCallback() {
    static uint32_t client_lockstep_counter = 0;
    static uint32_t burst_count = 0;
    // Increment the counter until all clients have received this burst. At that point we can either
    // send another burst, or stop the loop.
    if (++client_lockstep_counter == kClientsWithBurstsStarted) {
      client_lockstep_counter = 0;
      if (++burst_count == kReceiveBurstCount) {
        burst_count = 0;
        loop().Quit();
      } else {
        fake_driver_.SendBurst();
      }
    }
  }
};

TEST_F(RadarReaderProxyOnBurstTest, DISABLED_OnlyStartedClientsReceiveBursts) {
  // Clients 0 and 1 start bursts. Verify that client 2 does not receive bursts.

  EXPECT_TRUE(radar_clients_[0]->StartBursts().is_ok());
  EXPECT_TRUE(radar_clients_[1]->StartBursts().is_ok());
  loop().RunUntilIdle();

  fake_driver_.SendBurst();

  loop().Run();

  EXPECT_EQ(radar_clients_[0].burst_count, 10);
  EXPECT_EQ(radar_clients_[1].burst_count, 10);
  EXPECT_EQ(radar_clients_[2].burst_count, 0);
}

TEST_F(RadarReaderProxyOnBurstTest, DISABLED_DriverErrorsSentToStartedClients) {
  // Clients 1 and 2 start bursts. The driver reports a burst error, which gets sent to only to
  // these clients.

  EXPECT_TRUE(radar_clients_[1]->StartBursts().is_ok());
  EXPECT_TRUE(radar_clients_[2]->StartBursts().is_ok());
  loop().RunUntilIdle();

  fake_driver_.SendError(fuchsia_hardware_radar::StatusCode::kSensorError);
  fake_driver_.SendBurst();

  loop().Run();

  EXPECT_EQ(radar_clients_[0].burst_count, 0);
  EXPECT_EQ(radar_clients_[1].burst_count, 10);
  EXPECT_EQ(radar_clients_[2].burst_count, 10);

  EXPECT_EQ(radar_clients_[0].error_status, fuchsia_hardware_radar::StatusCode::kSuccess);
  EXPECT_EQ(radar_clients_[1].error_status, fuchsia_hardware_radar::StatusCode::kSensorError);
  EXPECT_EQ(radar_clients_[2].error_status, fuchsia_hardware_radar::StatusCode::kSensorError);
}

TEST_F(RadarReaderProxyOnBurstTest, DISABLED_OnlyOffendingClientReceivesOutOfVmosError) {
  // Start bursts for client 3, which we configured to not unlock VMOs. Client 3 should get a burst
  // for every registered VMO (five), then a kOutOfVmos error. Other clients should continue to get
  // bursts delivered like normal.

  EXPECT_TRUE(radar_clients_[1]->StartBursts().is_ok());
  EXPECT_TRUE(radar_clients_[2]->StartBursts().is_ok());
  EXPECT_TRUE(radar_clients_[3]->StartBursts().is_ok());
  loop().RunUntilIdle();

  fake_driver_.SendBurst();

  loop().Run();

  EXPECT_EQ(radar_clients_[1].burst_count, 10);
  EXPECT_EQ(radar_clients_[2].burst_count, 10);
  EXPECT_EQ(radar_clients_[3].burst_count, 5);

  EXPECT_EQ(radar_clients_[1].error_status, fuchsia_hardware_radar::StatusCode::kSuccess);
  EXPECT_EQ(radar_clients_[2].error_status, fuchsia_hardware_radar::StatusCode::kSuccess);
  EXPECT_EQ(radar_clients_[3].error_status, fuchsia_hardware_radar::StatusCode::kOutOfVmos);
}

TEST_F(RadarReaderProxyOnBurstTest, DISABLED_DisconnectedClientStopsReceivingBursts) {
  // Client 1 disconnects, now clients 0 and 2 get bursts.

  EXPECT_TRUE(radar_clients_[0]->StartBursts().is_ok());
  EXPECT_TRUE(radar_clients_[1]->StartBursts().is_ok());
  EXPECT_TRUE(radar_clients_[2]->StartBursts().is_ok());
  loop().RunUntilIdle();

  radar_clients_[1].client = fidl::Client<fuchsia_hardware_radar::RadarBurstReader>();
  loop().RunUntilIdle();

  fake_driver_.SendBurst();

  loop().Run();

  EXPECT_EQ(radar_clients_[0].burst_count, 10);
  EXPECT_EQ(radar_clients_[1].burst_count, 0);
  EXPECT_EQ(radar_clients_[2].burst_count, 10);
}

TEST_F(RadarReaderProxyOnBurstTest, DISABLED_BurstsStoppedAfterAllClientsDisconnect) {
  // All clients disconnect, bursts should be stopped.

  EXPECT_TRUE(radar_clients_[0]->StartBursts().is_ok());
  EXPECT_TRUE(radar_clients_[1]->StartBursts().is_ok());
  EXPECT_TRUE(radar_clients_[2]->StartBursts().is_ok());
  loop().RunUntilIdle();

  fake_driver_.SendBurst();

  radar_clients_.clear();
  loop().RunUntilIdle();

  EXPECT_OK(fake_driver_.WaitForBurstsStopped());
}

TEST_F(RadarReaderProxyOnBurstTest, DISABLED_StoppedClientStopsReceivingBursts) {
  // A client that stops bursts should not receive any more bursts.

  EXPECT_TRUE(radar_clients_[0]->StartBursts().is_ok());
  EXPECT_TRUE(radar_clients_[1]->StartBursts().is_ok());
  loop().RunUntilIdle();

  fake_driver_.SendBurst();

  loop().Run();
  loop().ResetQuit();

  EXPECT_EQ(radar_clients_[0].burst_count, 10);
  EXPECT_EQ(radar_clients_[1].burst_count, 10);
  EXPECT_EQ(radar_clients_[2].burst_count, 0);

  radar_clients_[0].burst_count = 0;
  radar_clients_[1].burst_count = 0;
  radar_clients_[2].burst_count = 0;

  radar_clients_[1]->StopBursts().Then([&](auto& result) { EXPECT_TRUE(result.is_ok()); });
  EXPECT_TRUE(radar_clients_[2]->StartBursts().is_ok());
  loop().RunUntilIdle();

  fake_driver_.SendBurst();

  loop().Run();

  EXPECT_EQ(radar_clients_[0].burst_count, 10);
  EXPECT_EQ(radar_clients_[1].burst_count, 0);
  EXPECT_EQ(radar_clients_[2].burst_count, 10);
}

class RadarReaderProxyInjectionTest : public RadarReaderProxyTest {
 public:
  void SetUp() override {
    RadarReaderProxyTest::SetUp();

    fake_driver_.set_burst_size(16);

    AddRadarDevice();

    {
      auto endpoints = fidl::Endpoints<fuchsia_hardware_radar::RadarBurstReader>::Create();
      reader_client_.emplace(std::move(endpoints.client), loop().dispatcher(),
                             fit::bind_member<&RadarReaderProxyInjectionTest::OnBurst>(this));
      dut_client_->Connect(std::move(endpoints.server)).Then([&](auto& result) {
        EXPECT_TRUE(result.is_ok());
      });
    }

    {
      std::vector<uint32_t> vmo_ids;
      std::vector<zx::vmo> vmos;
      for (uint32_t i = 0; i < 15; i++) {
        EXPECT_OK(zx::vmo::create(16, 0, &vmos.emplace_back()));
        EXPECT_OK(vmos.back().duplicate(ZX_RIGHT_SAME_RIGHTS, &registered_vmos_.emplace_back()));
        vmo_ids.emplace_back(i + 1);
      }

      reader_client_.value()
          ->RegisterVmos({std::move(vmo_ids), std::move(vmos)})
          .Then([](auto& result) { EXPECT_TRUE(result.is_ok()); });
    }

    {
      auto endpoints = fidl::Endpoints<fuchsia_hardware_radar::RadarBurstInjector>::Create();
      injector_client_.emplace(std::move(endpoints.client), loop().dispatcher(),
                               [&](uint32_t bursts_id) {
                                 received_vmo_ids_.push_back(bursts_id);
                                 if (on_bursts_delivered_) {
                                   on_bursts_delivered_();
                                 }
                               });
      BindInjector(std::move(endpoints.server));
    }
  }

 protected:
  struct BurstInjectorClient : fidl::AsyncEventHandler<fuchsia_hardware_radar::RadarBurstInjector> {
   public:
    BurstInjectorClient(fidl::ClientEnd<fuchsia_hardware_radar::RadarBurstInjector> client_end,
                        async_dispatcher_t* dispatcher, std::function<void(uint32_t)> callback)
        : client(std::move(client_end), dispatcher, this),
          bursts_delivered_callback(std::move(callback)) {}

    void OnBurstsDelivered(
        fidl::Event<fuchsia_hardware_radar::RadarBurstInjector::OnBurstsDelivered>& event)
        override {
      bursts_delivered_callback(event.bursts_id());
    }

    fidl::Client<fuchsia_hardware_radar::RadarBurstInjector>& operator->() { return client; }

    fidl::Client<fuchsia_hardware_radar::RadarBurstInjector> client;
    fuchsia_hardware_radar::StatusCode error_status = fuchsia_hardware_radar::StatusCode::kSuccess;

   private:
    const std::function<void(uint32_t)> bursts_delivered_callback;
  };

  fidl::Request<fuchsia_hardware_radar::RadarBurstInjector::EnqueueBursts> CreateInjectionVmo(
      uint8_t count, uint8_t offset = 0) {
    zx::vmo vmo;
    EXPECT_OK(zx::vmo::create(count * 16, 0, &vmo));

    for (uint8_t i = 0; i < count; i++) {
      uint8_t byte = i + offset;
      EXPECT_OK(vmo.write(&byte, i * 16, sizeof(byte)));
    }

    return {{std::move(vmo), count}};
  }

  std::optional<BurstReaderClient> reader_client_;
  std::optional<BurstInjectorClient> injector_client_;
  std::vector<uint8_t> received_burst_headers_;
  std::vector<zx::vmo> registered_vmos_;
  std::vector<uint32_t> received_vmo_ids_;
  fit::function<void(size_t)> on_burst_;
  fit::function<void(void)> on_bursts_delivered_;

 private:
  void OnBurst(uint32_t vmo_id) {
    ASSERT_GE(vmo_id, 1);
    ASSERT_LE(vmo_id, 15);

    uint8_t byte = 0;
    EXPECT_OK(registered_vmos_[vmo_id - 1].read(&byte, 0, sizeof(byte)));
    received_burst_headers_.push_back(byte);

    if (on_burst_) {
      on_burst_(received_burst_headers_.size());
    }
  }
};

TEST_F(RadarReaderProxyInjectionTest, DISABLED_InjectBursts) {
  on_burst_ = [&](size_t burst_count) {
    if (burst_count == 2) {
      // Start burst injection after two real bursts.
      injector_client_.value()->StartBurstInjection().Then(
          [&](auto& result) { EXPECT_TRUE(result.is_ok()); });

      // Immediately send a request to stop injection, which will get completed after all three
      // burst buffers (containing a total of eight bursts) have been sent.
      injector_client_.value()->StopBurstInjection().Then([&](auto& result) {
        EXPECT_TRUE(result.is_ok());
        // Send a burst from the driver to continue the test from the burst callback.
        fake_driver_.SendBurst();
      });
    } else if (burst_count == 12) {
      // Stop the loop after 12 total bursts.
      loop().Quit();
    } else if (burst_count < 2 || burst_count > 10) {
      fake_driver_.SendBurst();
    }
  };

  EXPECT_TRUE(reader_client_.value()->StartBursts().is_ok());
  injector_client_.value()->EnqueueBursts(CreateInjectionVmo(3, 0)).Then([&](auto& result) {
    EXPECT_TRUE(result.is_ok());
  });
  injector_client_.value()->EnqueueBursts(CreateInjectionVmo(2, 3)).Then([&](auto& result) {
    EXPECT_TRUE(result.is_ok());
  });
  injector_client_.value()->EnqueueBursts(CreateInjectionVmo(3, 5)).Then([&](auto& result) {
    EXPECT_TRUE(result.is_ok());
  });
  loop().RunUntilIdle();

  fake_driver_.SendBurst();

  loop().Run();

  const std::vector<uint8_t> kExpectedBurstHeaders = {0xff, 0xff, 0x00, 0x01, 0x02, 0x03,
                                                      0x04, 0x05, 0x06, 0x07, 0xff, 0xff};
  ASSERT_EQ(received_burst_headers_.size(), kExpectedBurstHeaders.size());
  EXPECT_EQ(received_burst_headers_, kExpectedBurstHeaders);

  const std::vector<uint32_t> kExpectedVmoIds = {1, 2, 3};
  ASSERT_EQ(received_vmo_ids_.size(), kExpectedVmoIds.size());
  EXPECT_EQ(received_vmo_ids_, kExpectedVmoIds);
}

TEST_F(RadarReaderProxyInjectionTest, DISABLED_InjectionPausedWithNoEnqueuedVmos) {
  on_bursts_delivered_ = [&]() {
    if (received_vmo_ids_.size() == 1) {
      // The last burst from the original VMO has been received, now enqueue a new one. We should
      // still not miss any injected bursts.
      injector_client_.value()->EnqueueBursts(CreateInjectionVmo(2, 3)).Then([&](auto& result) {
        EXPECT_TRUE(result.is_ok());
      });
    } else if (received_vmo_ids_.size() == 2) {
      injector_client_.value()->StopBurstInjection().Then([&](auto& result) {
        EXPECT_TRUE(result.is_ok());
        // We know that the last bursts will have been sent on the reader client channel by the time
        // StopBurstInjection() replies. We can ensure that all bursts have been handled by sending
        // another request with the reader client then stopping the loop when the response is
        // received.
        reader_client_.value()->GetBurstProperties().Then([&](auto& result) { loop().Quit(); });
      });
    }
  };

  // Start burst injection, then enqueue a burst VMO. Clients should not miss any injected bursts.
  injector_client_.value()->StartBurstInjection().Then([&](auto& result) {
    EXPECT_TRUE(result.is_ok());
    EXPECT_TRUE(reader_client_.value()->StartBursts().is_ok());
    reader_client_.value()->GetBurstProperties().Then([&](auto& result) {
      injector_client_.value()->EnqueueBursts(CreateInjectionVmo(3, 0)).Then([&](auto& result) {
        EXPECT_TRUE(result.is_ok());
      });
    });
  });

  loop().Run();

  const std::vector<uint8_t> kExpectedBurstHeaders = {0, 1, 2, 3, 4};
  ASSERT_EQ(received_burst_headers_.size(), kExpectedBurstHeaders.size());
  EXPECT_EQ(received_burst_headers_, kExpectedBurstHeaders);

  // The buffer ID counter should be reset after the first one is returned to us.
  const std::vector<uint32_t> kExpectedVmoIds = {1, 1};
  ASSERT_EQ(received_vmo_ids_.size(), kExpectedVmoIds.size());
  EXPECT_EQ(received_vmo_ids_, kExpectedVmoIds);
}

TEST_F(RadarReaderProxyInjectionTest, DISABLED_BurstsNotDeliveredToStopppedClients) {
  zx::result endpoints = fidl::CreateEndpoints<fuchsia_hardware_radar::RadarBurstReader>();
  ASSERT_TRUE(endpoints.is_ok());

  dut_client_->Connect(std::move(endpoints->server)).Then([&](auto& result) {
    EXPECT_TRUE(result.is_ok());
  });

  BurstReaderClient started_client(std::move(endpoints->client), loop().dispatcher(),
                                   [&](uint32_t) {
                                     if (started_client.burst_count == 3) {
                                       loop().Quit();
                                     }
                                   });

  for (uint32_t i = 0; i < 3; i++) {
    std::vector<zx::vmo> vmos;
    EXPECT_OK(zx::vmo::create(16, 0, &vmos.emplace_back()));
    started_client->RegisterVmos({std::vector<uint32_t>{i}, std::move(vmos)})
        .Then([&](auto& result) { EXPECT_TRUE(result.is_ok()); });
  }

  EXPECT_TRUE(started_client->StartBursts().is_ok());
  started_client->GetBurstProperties().Then([&](auto& result) {
    injector_client_.value()->EnqueueBursts(CreateInjectionVmo(3, 0)).Then([&](auto& result) {
      EXPECT_TRUE(result.is_ok());
    });
    injector_client_.value()->StartBurstInjection().Then(
        [&](auto& result) { EXPECT_TRUE(result.is_ok()); });
  });

  loop().Run();

  // The original (stopped) client should receive zero bursts, while the started client should
  // receive three.
  EXPECT_EQ(reader_client_->burst_count, 0);
  EXPECT_EQ(started_client.burst_count, 3);
}

TEST_F(RadarReaderProxyInjectionTest, DISABLED_BurstsReceivedDuringInjectionAreIgnored) {
  on_burst_ = [&](size_t) {
    if (received_burst_headers_.size() == 5 && received_vmo_ids_.size() == 1) {
      loop().Quit();
    }
  };
  on_bursts_delivered_ = [&]() {
    if (received_burst_headers_.size() == 5 && received_vmo_ids_.size() == 1) {
      loop().Quit();
    }
  };

  injector_client_.value()->EnqueueBursts(CreateInjectionVmo(5)).Then([&](auto& result) {
    EXPECT_TRUE(result.is_ok());
  });

  EXPECT_TRUE(reader_client_.value()->StartBursts().is_ok());
  reader_client_.value()->GetBurstProperties().Then([&](auto& result) {
    injector_client_.value()->StartBurstInjection().Then([&](auto& result) {
      EXPECT_TRUE(result.is_ok());
      // Send three bursts from the driver after injection has been enabled and the client has been
      // started. All three should be ignored in favor of the injected bursts.
      fake_driver_.SendBurst();
      fake_driver_.SendBurst();
      fake_driver_.SendBurst();
    });
  });

  loop().Run();

  const std::vector<uint8_t> kExpectedBursts = {0, 1, 2, 3, 4};
  ASSERT_EQ(received_burst_headers_.size(), kExpectedBursts.size());
  EXPECT_EQ(received_burst_headers_, kExpectedBursts);

  const std::vector<uint32_t> kExpectedVmoIds = {1};
  ASSERT_EQ(received_vmo_ids_.size(), kExpectedVmoIds.size());
  EXPECT_EQ(received_vmo_ids_, kExpectedVmoIds);
}

TEST_F(RadarReaderProxyInjectionTest, DISABLED_CallsFailsWithBadState) {
  injector_client_.value()->StartBurstInjection().Then(
      [&](auto& result) { EXPECT_TRUE(result.is_ok()); });

  injector_client_.value()->StartBurstInjection().Then([&](auto& result) {
    ASSERT_FALSE(result.is_ok());
    ASSERT_TRUE(result.error_value().is_domain_error());
    EXPECT_EQ(result.error_value().domain_error(), fuchsia_hardware_radar::StatusCode::kBadState);
  });

  injector_client_.value()->StopBurstInjection().Then(
      [&](auto& result) { EXPECT_TRUE(result.is_ok()); });

  injector_client_.value()->StopBurstInjection().Then([&](auto& result) {
    ASSERT_FALSE(result.is_ok());
    ASSERT_TRUE(result.error_value().is_domain_error());
    EXPECT_EQ(result.error_value().domain_error(), fuchsia_hardware_radar::StatusCode::kBadState);

    loop().Quit();
  });

  loop().Run();
}

TEST_F(RadarReaderProxyInjectionTest, DISABLED_OnlyOneInjectorCanConnect) {
  struct : public fidl::AsyncEventHandler<fuchsia_hardware_radar::RadarBurstInjector> {
    void on_fidl_error(fidl::UnbindInfo info) override {
      EXPECT_EQ(info.status(), ZX_ERR_ALREADY_BOUND);
      fidl_error_callback();
    }
    fit::callback<void(void)> fidl_error_callback;
  } client_disconnect_waiter;
  client_disconnect_waiter.fidl_error_callback = [&]() { loop().Quit(); };

  zx::result endpoints = fidl::CreateEndpoints<fuchsia_hardware_radar::RadarBurstInjector>();
  ASSERT_TRUE(endpoints.is_ok());

  BindInjector(std::move(endpoints->server));

  fidl::Client<fuchsia_hardware_radar::RadarBurstInjector> client(
      std::move(endpoints->client), loop().dispatcher(), &client_disconnect_waiter);

  loop().Run();
}

TEST_F(RadarReaderProxyTest, DISABLED_ConnectInjectorBeforeRadarDevice) {
  zx::result endpoints = fidl::CreateEndpoints<fuchsia_hardware_radar::RadarBurstInjector>();
  ASSERT_TRUE(endpoints.is_ok());

  BindInjector(std::move(endpoints->server));

  fidl::Client<fuchsia_hardware_radar::RadarBurstInjector> client(std::move(endpoints->client),
                                                                  loop().dispatcher());

  // Issue two requests before adding the radar device. They should be fulfilled after the proxy
  // connects.
  client->StartBurstInjection().Then([&](auto& result) { EXPECT_TRUE(result.is_ok()); });
  client->StopBurstInjection().Then([&](auto& result) {
    EXPECT_TRUE(result.is_ok());
    loop().Quit();
  });

  AddRadarDevice();

  loop().Run();
}

}  // namespace
}  // namespace radar
