// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/test.protocol.connector/cpp/fidl.h>
#include <fidl/test.protocol.connector/cpp/hlcpp_conversion.h>
#include <lib/fidl/cpp/wire/channel.h>

#include <gtest/gtest.h>

#ifndef __Fuchsia__
#error This test only makes sense on Fuchsia.
#endif

TEST(ProtocolHandlerConversion, ToNatural) {
  zx_handle_t hlcpp_server_handle;
  fidl::InterfaceRequestHandler<test::protocol::connector::SimpleProtocol>
      interface_request_handler =
          [&hlcpp_server_handle](
              fidl::InterfaceRequest<test::protocol::connector::SimpleProtocol> request) {
            hlcpp_server_handle = request.channel().get();
          };

  fidl::ProtocolHandler<test_protocol_connector::SimpleProtocol> protocol_handler =
      fidl::HLCPPToNatural(std::move(interface_request_handler));

  auto natural_endpoints = fidl::Endpoints<test_protocol_connector::SimpleProtocol>::Create();
  zx_handle_t natural_server_handle = natural_endpoints.server.channel().get();

  protocol_handler(std::move(natural_endpoints.server));

  EXPECT_EQ(hlcpp_server_handle, natural_server_handle);
}

TEST(ProtocolHandlerConversion, ToHLCPP) {
  zx_handle_t natural_server_handle;
  fidl::ProtocolHandler<test_protocol_connector::SimpleProtocol> protocol_handler =
      [&natural_server_handle](
          fidl::ServerEnd<test_protocol_connector::SimpleProtocol> server_end) {
        natural_server_handle = server_end.channel().get();
      };

  fidl::InterfaceRequestHandler<test::protocol::connector::SimpleProtocol>
      interface_request_handler = fidl::NaturalToHLCPP(std::move(protocol_handler));

  fidl::InterfaceHandle<test::protocol::connector::SimpleProtocol> hlcpp_client;
  auto hlcpp_server = hlcpp_client.NewRequest();
  zx_handle_t hlcpp_server_handle = hlcpp_server.channel().get();

  interface_request_handler(std::move(hlcpp_server));

  EXPECT_EQ(hlcpp_server_handle, natural_server_handle);
}
