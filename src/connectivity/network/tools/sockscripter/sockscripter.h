// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_NETWORK_TOOLS_SOCKSCRIPTER_SOCKSCRIPTER_H_
#define SRC_CONNECTIVITY_NETWORK_TOOLS_SOCKSCRIPTER_SOCKSCRIPTER_H_

#include <string>

#include "api_abstraction.h"
#include "packet.h"

// {sendto}N=3T=2 1.2.3.4:3445
// "{<cmd>}[N=<n>][T=<t>]"
struct TestRepeatCfg {
  std::string command;
  int repeat_count = 1;
  int delay_ms = 0;

  bool Parse(const std::string& cmd);
};

class SendBufferGenerator {
 public:
  enum Mode { STATIC_TEXT, COUNTER_TEXT };
  static constexpr const char* kCounterText = "Packet number %c.";

  SendBufferGenerator() : snd_str_(kCounterText), mode_(COUNTER_TEXT) {}

  std::string GetSndStr();
  bool SetSendBufHex(const char* arg);
  bool SetSendBufText(const char* arg);
  bool SetSendBufLen(size_t len);

 private:
  std::string snd_str_;
  Mode mode_ = STATIC_TEXT;
  uint32_t counter_ = 0;
};

class SockScripter {
 public:
  explicit SockScripter(ApiAbstraction* abstraction) : api_(abstraction) {}

  int Execute(int argc, char* const argv[]);

  struct sockaddr* GetSockAddrStorage();
  bool Open(int domain, int type, int proto);
  bool Close(char* arg);
  bool CloseListener(char* arg);
  bool SetBroadcast(char* arg);
  bool LogBroadcast(char* arg);
  bool SetRecvtclass(char* arg);
  bool LogRecvtclass(char* arg);
  bool SetTcpSyncnt(char* arg);
  bool LogTcpSyncnt(char* arg);
  bool SetTcpUserTimeout(char* arg);
  bool LogTcpUserTimeout(char* arg);
  bool SetRecvtos(char* arg);
  bool LogRecvtos(char* arg);
  bool SetReuseaddr(char* arg);
  bool LogReuseaddr(char* arg);
  bool SetReuseport(char* arg);
  bool LogReuseport(char* arg);
  bool SetIpUnicastTTL(char* arg);
  bool LogIpUnicastTTL(char* arg);
  bool SetIpUnicastHops(char* arg);
  bool LogIpUnicastHops(char* arg);
  bool SetIpMcastTTL(char* arg);
  bool LogIpMcastTTL(char* arg);
  bool SetIpMcastLoop4(char* arg);
  bool LogIpMcastLoop4(char* arg);
  bool SetIpMcastHops(char* arg);
  bool LogIpMcastHops(char* arg);
  bool SetIpV6Only(char* arg);
  bool LogIpV6Only(char* arg);
  bool SetIpMcastLoop6(char* arg);
  bool LogIpMcastLoop6(char* arg);
  bool SetBindToDevice(char* arg);
  bool LogBindToDevice(char* arg);
  bool SetIpMcastIf4(char* arg);
  bool LogIpMcastIf4(char* arg);
  bool SetIpMcastIf6(char* arg);
  bool LogIpMcastIf6(char* arg);
  bool LogBoundToAddress(char* arg);
  bool LogPeerAddress(char* arg);
  bool SetIpRecvOrigDstAddr(char* arg);
  bool LogIpRecvOrigDstAddr(char* arg);
  bool SetIpv6RecvPktInfo(char* arg);
  bool LogIpv6RecvPktInfo(char* arg);
  bool SetIpTransparent(char* arg);
  bool LogIpTransparent(char* arg);
  bool SetIpHeaderInclude(char* arg);
  bool LogIpHeaderInclude(char* arg);
  bool SetIcmp6Filter(char* arg);
  bool LogIcmp6Filter(char* arg);
  bool LogOriginalDestination(char* arg);
  bool LogOriginalDestination6(char* arg);
  bool SetTClass(char* arg);
  bool LogTClass(char* arg);
  bool SetTos(char* arg);
  bool LogTos(char* arg);
  bool LogError(char* arg);
  bool Bind(char* arg);
  bool Shutdown(char* arg);
  bool Connect(char* arg);
  bool Disconnect(char* arg);
  bool Join4(char* arg);
  bool Drop4(char* arg);
  bool Block4(char* arg);
  bool Join6(char* arg);
  bool Drop6(char* arg);
  bool Listen(char* arg);
  bool Accept(char* arg);
  bool SendTo(char* arg);
  bool Send(char* arg);
  bool RecvFromInternal(bool ping);
  bool RecvFrom(char* arg);
  bool RecvFromPing(char* arg);
  int RecvInternal(bool ping);
  bool Recv(char* arg);
  bool RecvPing(char* arg);
  bool Recvmsg(char* arg);
  bool SetSendBufHex(char* arg);
  bool SetSendBufText(char* arg);
  bool SetSendBufLen(char* arg);
  bool Sleep(char* arg);

#if PACKET_SOCKETS
  bool PacketBind(char* arg);
  bool PacketSendTo(char* arg);
#endif

 private:
  bool JoinOrDrop4(const char* func, char* arg, int optname, const char* optname_str);
  bool JoinOrDrop6(const char* func, char* arg, int optname, const char* optname_str);

  int sockfd_ = -1;
  int tcp_listen_socket_fd_ = -1;
  int snd_flags_ = 0;
  int recv_flags_ = 0;
  SendBufferGenerator snd_buf_gen_;
  char recv_buf_[1024]{};
  ApiAbstraction* api_;
  bool continue_after_error_ = false;
};

#endif  // SRC_CONNECTIVITY_NETWORK_TOOLS_SOCKSCRIPTER_SOCKSCRIPTER_H_
