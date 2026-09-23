/*
 * Copyright (c) 2019 The Fuchsia Authors
 *
 * Permission to use, copy, modify, and/or distribute this software for any
 * purpose with or without fee is hereby granted, provided that the above
 * copyright notice and this permission notice appear in all copies.
 *
 * THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
 * WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
 * MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY
 * SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
 * WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION
 * OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN
 * CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
 */
#include <zircon/errors.h>

#include "fidl/fuchsia.wlan.phy/cpp/wire_types.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/test/sim_test.h"

namespace wlan::brcmfmac {
namespace {

std::pair<zx::channel, zx::channel> make_channel() {
  zx::channel local;
  zx::channel remote;
  zx::channel::create(0, &local, &remote);
  return {std::move(local), std::move(remote)};
}

class LifecycleTest : public SimTest {
  void SetUp() override { ASSERT_OK(SimTest::Init()); }
};

TEST_F(LifecycleTest, CreateIfaceInvalidArgsFails) {
  fuchsia_wlan_phy::WlanPhyCreateIfaceRequest req;
  auto result = client_->CreateIface(std::move(req));

  ASSERT_TRUE(result.is_error());
  ASSERT_TRUE(result.error_value().is_domain_error());
  ASSERT_EQ(result.error_value().domain_error(), ZX_ERR_INVALID_ARGS);
}

TEST_F(LifecycleTest, CreateIfaceInvalidRoleFails) {
  auto [local, _remote] = make_channel();
  fuchsia_wlan_phy::WlanPhyCreateIfaceRequest req{{
      .role = static_cast<wlan_common::WlanMacRole>(999),
      .mlme_channel = std::move(local),
  }};
  auto result = client_->CreateIface(std::move(req));

  ASSERT_TRUE(result.is_error());
  ASSERT_TRUE(result.error_value().is_domain_error());
  ASSERT_EQ(result.error_value().domain_error(), ZX_ERR_INVALID_ARGS);
}

TEST_F(LifecycleTest, CreateMultipleClientIfaceFails) {
  uint16_t iface_id = 0;
  {
    // First call to create iface with client role should succeed
    auto [local, _remote] = make_channel();
    fuchsia_wlan_phy::WlanPhyCreateIfaceRequest req{{
        .role = wlan_common::WlanMacRole::kClient,
        .mlme_channel = std::move(local),
    }};
    auto result = client_->CreateIface(std::move(req));
    ASSERT_TRUE(result.is_ok());
    iface_id = result->iface_id().value();
  }

  {
    // Second call to create iface with client role fails
    auto [local, _remote] = make_channel();
    fuchsia_wlan_phy::WlanPhyCreateIfaceRequest req{{
        .role = wlan_common::WlanMacRole::kClient,
        .mlme_channel = std::move(local),
    }};
    auto result = client_->CreateIface(std::move(req));
    ASSERT_TRUE(result.is_error());
    ASSERT_TRUE(result.error_value().is_domain_error());
    ASSERT_EQ(result.error_value().domain_error(), ZX_ERR_NO_RESOURCES);
  }

  {
    // destroy client iface manually to clean up
    fuchsia_wlan_phy::WlanPhyDestroyIfaceRequest req{{.iface_id = iface_id}};

    auto result = client_->DestroyIface(req);
    ASSERT_TRUE(result.is_ok());
  }
}

TEST_F(LifecycleTest, DestroyIfaceWithoutIfaceIdFails) {
  uint16_t iface_id = 0;
  {
    // First call to create iface with client role should succeed
    auto [local, _remote] = make_channel();
    fuchsia_wlan_phy::WlanPhyCreateIfaceRequest req{{
        .role = wlan_common::WlanMacRole::kClient,
        .mlme_channel = std::move(local),
    }};
    auto result = client_->CreateIface(std::move(req));
    ASSERT_TRUE(result.is_ok());
    iface_id = result->iface_id().value();
  }

  {
    // destroy client iface without iface_id fails
    fuchsia_wlan_phy::WlanPhyDestroyIfaceRequest req;

    auto result = client_->DestroyIface(req);
    ASSERT_TRUE(result.is_error());
    ASSERT_TRUE(result.error_value().is_domain_error());
    ASSERT_EQ(result.error_value().domain_error(), ZX_ERR_INVALID_ARGS);
  }

  {
    // destroy client iface manually to clean up
    fuchsia_wlan_phy::WlanPhyDestroyIfaceRequest req{{.iface_id = iface_id}};

    auto result = client_->DestroyIface(req);
    ASSERT_TRUE(result.is_ok());
  }
}

TEST_F(LifecycleTest, FullmacInitMultipleCallsFails) {
  uint16_t iface_id = 0;
  {
    // First call to create iface with client role should succeed
    auto [local, _remote] = make_channel();
    fuchsia_wlan_phy::WlanPhyCreateIfaceRequest req{{
        .role = wlan_common::WlanMacRole::kClient,
        .mlme_channel = std::move(local),
    }};
    auto result = client_->CreateIface(std::move(req));
    ASSERT_TRUE(result.is_ok());
    iface_id = result->iface_id().value();
  }

  {
    zx::result driver_connect_result =
        component::ConnectAtMember<fuchsia_wlan_fullmac::Service::WlanFullmacImpl>(
            CreateDriverSvcClient(), "brcmfmac-wlan-fullmac-client");
    EXPECT_EQ(ZX_OK, driver_connect_result.status_value());

    auto fullmac_client = fidl::WireSyncClient<fuchsia_wlan_fullmac::WlanFullmacImpl>(
        std::move(driver_connect_result.value()));

    {
      // first call to init succeeds
      auto endpoints = fidl::CreateEndpoints<fuchsia_wlan_fullmac::WlanFullmacImplIfc>();
      auto req = fuchsia_wlan_fullmac::wire::WlanFullmacImplInitRequest::Builder(test_arena_)
                     .ifc(std::move(endpoints->client))
                     .Build();
      auto result = fullmac_client.buffer(test_arena_)->Init(req);
      ASSERT_TRUE(result.ok() && !result->is_error());
    }

    {
      // second call to init fails
      auto endpoints = fidl::CreateEndpoints<fuchsia_wlan_fullmac::WlanFullmacImplIfc>();
      auto req = fuchsia_wlan_fullmac::wire::WlanFullmacImplInitRequest::Builder(test_arena_)
                     .ifc(std::move(endpoints->client))
                     .Build();
      auto result = fullmac_client.buffer(test_arena_)->Init(req);
      ASSERT_EQ(result->error_value(), ZX_ERR_ALREADY_BOUND);
    }
  }

  {
    // destroy client iface manually to clean up
    fuchsia_wlan_phy::WlanPhyDestroyIfaceRequest req{{.iface_id = iface_id}};

    auto result = client_->DestroyIface(req);
    ASSERT_TRUE(result.is_ok());
  }
}

}  // namespace
}  // namespace wlan::brcmfmac
