// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <arpa/inet.h>
#include <lib/syslog/cpp/macros.h>
#include <netinet/icmp6.h>
#include <netinet/ip_icmp.h>
#include <netinet/tcp.h>
#include <sys/socket.h>

#include <array>
#include <iostream>
#include <vector>

#include <fbl/unique_fd.h>
#include <perftest/perftest.h>

#include "src/connectivity/network/tests/os.h"
#include "src/lib/fxl/strings/string_printf.h"

#if defined(__Fuchsia__)
#include <lib/trace/event.h>

#include "src/performance/lib/test_utils/trace_controller.h"
#endif

namespace {

#define CHECK_TRUE_ERRNO(true_condition) FX_CHECK(true_condition) << strerror(errno)
#define CHECK_ZERO_ERRNO(value)                                                          \
  do {                                                                                   \
    auto c = (value);                                                                    \
    FX_CHECK(c == 0) << "expected zero, got " << c << " with errno " << strerror(errno); \
  } while (0)

#define CHECK_POSITIVE(value)                            \
  do {                                                   \
    if (auto c = (value); c <= 0) {                      \
      FX_CHECK(c != 0) << "expected nonzero, got " << c; \
      FX_LOGS(FATAL) << strerror(errno);                 \
    }                                                    \
  } while (0)

constexpr char kFakeNetstackEnvVar[] = "FAKE_NETSTACK";
constexpr char kNetstack3EnvVar[] = "NETSTACK3";
constexpr char kStarnixEnvVar[] = "STARNIX";
#if defined(__Fuchsia__)
constexpr char kSocketBenchmarksTracingCategory[] = "socket_benchmarks";
constexpr char kTracingEnvVar[] = "TRACING";
#endif

template <typename T>
class AddrStorage {
 public:
  static_assert(std::is_same_v<T, sockaddr_in> || std::is_same_v<T, sockaddr_in6>);
  sockaddr* as_sockaddr() { return reinterpret_cast<sockaddr*>(&addr); }
  const sockaddr* as_sockaddr() const { return reinterpret_cast<const sockaddr*>(&addr); }
  socklen_t socklen() const { return sizeof(addr); }
  T addr;
};

class Ipv6 {
 public:
  using SockAddr = AddrStorage<sockaddr_in6>;
  static constexpr int kFamily = AF_INET6;
  static constexpr int kIpProtoIcmp = IPPROTO_ICMPV6;
  static constexpr uint8_t kIcmpEchoRequestType = ICMP6_ECHO_REQUEST;
  static constexpr uint8_t kIcmpEchoReplyType = ICMP6_ECHO_REPLY;

  static SockAddr loopback() {
    return {
        .addr =
            {
                .sin6_family = kFamily,
                .sin6_addr = IN6ADDR_LOOPBACK_INIT,
            },
    };
  }
};

class Ipv4 {
 public:
  using SockAddr = AddrStorage<sockaddr_in>;
  static constexpr int kFamily = AF_INET;
  static constexpr int kIpProtoIcmp = IPPROTO_ICMP;
  static constexpr uint8_t kIcmpEchoRequestType = ICMP_ECHO;
  static constexpr uint8_t kIcmpEchoReplyType = ICMP_ECHOREPLY;

  static SockAddr loopback() {
    return {
        .addr =
            {
                .sin_family = kFamily,
                .sin_addr =
                    {
                        .s_addr = htonl(INADDR_LOOPBACK),
                    },
            },
    };
  }
};

enum class BufferSizeType { kTcpSend, kUdpRecv };

int ExpectedGetBufferSizeFuchsia(int set_size, BufferSizeType buffer_type) {
  if (std::getenv(kNetstack3EnvVar)) {
    switch (buffer_type) {
      case BufferSizeType::kTcpSend:
        return std::clamp(set_size, 2048, 4 << 20);
      case BufferSizeType::kUdpRecv:
        return set_size;
    }
  }
  return set_size;
}

int ExpectedGetBufferSize(int set_size, BufferSizeType buffer_type) {
  // The desired return value for getting SO_SNDBUF and SO_RCVBUF on Linux is
  // double the amount of payload bytes due to the fact that the value is
  // doubled on set to account for overhead according to the [man page].
  //
  // [man page]: https://man7.org/linux/man-pages/man7/socket.7.html
#ifdef __linux__
  // If running on Starnix, the expected value should actually be that of
  // Fuchsia's, and not Linux's.
  if (std::getenv(kStarnixEnvVar)) {
    return ExpectedGetBufferSizeFuchsia(set_size, buffer_type);
  }
  set_size *= 2;
  // NB: This minimum is a magic number and seems to contradict the stated
  // minimum in the Linux man page of 2048 for SNDBUF.
  if (set_size < 4608) {
    return 4608;
  }
  return set_size;
#endif
  return ExpectedGetBufferSizeFuchsia(set_size, buffer_type);
}

// Helper no-op function to assert functions abstracted over IP version are properly parameterized.
template <typename Ip>
void TemplateIsIpVersion() {
  static_assert(std::is_same_v<Ip, Ipv4> || std::is_same_v<Ip, Ipv6>);
}

// Computes the unidirectional throughput on a TCP loopback socket.
//
// Measures the time to write `transfer` bytes on one end of the socket and read them on the other
// end on the same thread and calculates the throughput.
template <typename Ip>
bool TcpWriteRead(perftest::RepeatState* state, int transfer) {
  TemplateIsIpVersion<Ip>();
  using Addr = typename Ip::SockAddr;
  fbl::unique_fd listen_sock;
  CHECK_TRUE_ERRNO(listen_sock = fbl::unique_fd(socket(Ip::kFamily, SOCK_STREAM, 0)));
  Addr sockaddr = Ip::loopback();
  CHECK_ZERO_ERRNO(bind(listen_sock.get(), sockaddr.as_sockaddr(), sockaddr.socklen()));
  CHECK_ZERO_ERRNO(listen(listen_sock.get(), 0));

  socklen_t socklen = sockaddr.socklen();
  CHECK_ZERO_ERRNO(getsockname(listen_sock.get(), sockaddr.as_sockaddr(), &socklen));

  fbl::unique_fd client_sock;
  CHECK_TRUE_ERRNO(client_sock = fbl::unique_fd(socket(Ip::kFamily, SOCK_STREAM, 0)));

  constexpr int kBufferSizeMultipler = 4;
  // Set send buffer larger than transfer size to ensure we can write `transfer`
  // bytes before reading it on the other end. The multiplier allows the
  // receiver to delay acknowledgements but the transfer still proceeds. This is
  // especially impactful for small transfer sizes.
  int sndbuf = transfer * kBufferSizeMultipler;
  CHECK_ZERO_ERRNO(setsockopt(client_sock.get(), SOL_SOCKET, SO_SNDBUF, &sndbuf, sizeof(sndbuf)));
  {
    int sndbuf_opt;
    socklen_t sndbuf_optlen = sizeof(sndbuf_opt);
    CHECK_ZERO_ERRNO(
        getsockopt(client_sock.get(), SOL_SOCKET, SO_SNDBUF, &sndbuf_opt, &sndbuf_optlen));

    int want_sndbuf = ExpectedGetBufferSize(sndbuf, BufferSizeType::kTcpSend);
    FX_CHECK(sndbuf_opt == want_sndbuf)
        << "sndbuf size (" << sndbuf_opt << ") != want (" << want_sndbuf << ")";
  }
  // Disable the Nagle algorithm, it introduces artificial latency that defeats this test.
  const int32_t no_delay = 1;
  CHECK_ZERO_ERRNO(
      setsockopt(client_sock.get(), SOL_TCP, TCP_NODELAY, &no_delay, sizeof(no_delay)));

  // Also update the receive buffer size.
  //
  // This ensures fairness in the benchmark since TCP will base the window value
  // on the available receive buffer size and different numbers will skew the
  // test results.
  //
  // This is set on the listening socket, which is inherited by accepted sockets
  // on creation.
  //
  // We use a multiplier on the transfer size so silly window avoidance doesn't
  // kick in in-between test iterations which causes pollution in the results.
  //
  // We don't perform the getopt check here on return to reduce the amount of
  // change detectors on buffer sizes required here, since the buffer size is
  // not load-bearing for the test to complete successfully.
  int recvbuf = transfer * kBufferSizeMultipler;
  CHECK_ZERO_ERRNO(setsockopt(listen_sock.get(), SOL_SOCKET, SO_RCVBUF, &recvbuf, sizeof(recvbuf)));

  CHECK_ZERO_ERRNO(connect(client_sock.get(), sockaddr.as_sockaddr(), sockaddr.socklen()));

  fbl::unique_fd server_sock;
  CHECK_TRUE_ERRNO(server_sock = fbl::unique_fd(accept(listen_sock.get(), nullptr, nullptr)));

  std::vector<uint8_t> send_bytes, recv_bytes;
  // Avoid large memory regions with zeroes that can cause the system to try and reclaim pages from
  // us. For more information see Zircon page scanner and eviction strategies.
  send_bytes.resize(transfer, 0xAA);
  recv_bytes.resize(transfer, 0xBB);

  while (state->KeepRunning()) {
    for (ssize_t sent = 0; sent < transfer;) {
      ssize_t wr;
      {
#ifdef __Fuchsia__
        TRACE_DURATION(kSocketBenchmarksTracingCategory, "tcp_write");
#endif
        wr = write(client_sock.get(), send_bytes.data() + sent, transfer - sent);
      }
      CHECK_POSITIVE(wr);
      sent += wr;
    }
    for (ssize_t recv = 0; recv < transfer;) {
      ssize_t rd;
      {
#ifdef __Fuchsia__
        TRACE_DURATION(kSocketBenchmarksTracingCategory, "tcp_read");
#endif
        rd = read(server_sock.get(), recv_bytes.data() + recv, transfer - recv);
      }
      CHECK_POSITIVE(rd);
      recv += rd;
    }
  }

  return true;
}

// Computes unidirectional throughput on a UDP loopback socket.
//
// Measures the time to write `message_count` messages of size `message_size`
// bytes on one end of the socket and read them out on the other end on the
// same thread and calculates the throughput.
template <typename Ip>
bool UdpWriteRead(perftest::RepeatState* state, int message_size, int message_count) {
  TemplateIsIpVersion<Ip>();
  using Addr = typename Ip::SockAddr;

  fbl::unique_fd server_sock;
  CHECK_TRUE_ERRNO(server_sock = fbl::unique_fd(socket(Ip::kFamily, SOCK_DGRAM, 0)));
  Addr sockaddr = Ip::loopback();
  CHECK_ZERO_ERRNO(bind(server_sock.get(), sockaddr.as_sockaddr(), sockaddr.socklen()));

  int rcvbuf_opt;
  socklen_t rcvbuf_optlen = sizeof(rcvbuf_opt);
  CHECK_ZERO_ERRNO(
      getsockopt(server_sock.get(), SOL_SOCKET, SO_RCVBUF, &rcvbuf_opt, &rcvbuf_optlen));

  int want_rcvbuf = ExpectedGetBufferSize(message_size * message_count, BufferSizeType::kUdpRecv);
  // On Linux, payloads are stored with a fixed per-packet overhead. Linux
  // accounts for this overhead by setting the actual buffer size to double
  // the size set with SO_RCVBUF. This hack fails when SO_RCVBUF is small and
  // many packets are sent; avoid that case by setting RCVBUF only when the
  // bytes-to-be-sent exceed the default value (which is large).
  if (rcvbuf_opt < want_rcvbuf) {
    int rcv_bufsize = message_size * message_count;
    CHECK_ZERO_ERRNO(
        setsockopt(server_sock.get(), SOL_SOCKET, SO_RCVBUF, &rcv_bufsize, sizeof(rcv_bufsize)));
    CHECK_ZERO_ERRNO(
        getsockopt(server_sock.get(), SOL_SOCKET, SO_RCVBUF, &rcvbuf_opt, &rcvbuf_optlen));

    FX_CHECK(rcvbuf_opt == want_rcvbuf)
        << "rcvbuf size (" << rcvbuf_opt << ") != want (" << want_rcvbuf << ")";
  }

  socklen_t socklen = sockaddr.socklen();
  CHECK_ZERO_ERRNO(getsockname(server_sock.get(), sockaddr.as_sockaddr(), &socklen));

  fbl::unique_fd client_sock;
  CHECK_TRUE_ERRNO(client_sock = fbl::unique_fd(socket(Ip::kFamily, SOCK_DGRAM, 0)));

  // Always set the send buffer size so the benchmark is fair around UDP
  // blocking for all platforms. Similarly to receive buffer, we only change it
  // if it's smaller than what we need.
  int sndbuf_opt;
  socklen_t sndbuf_optlen = sizeof(sndbuf_opt);
  int want_sndbuf = message_size * message_count;
  CHECK_ZERO_ERRNO(
      getsockopt(client_sock.get(), SOL_SOCKET, SO_SNDBUF, &sndbuf_opt, &sndbuf_optlen));
  if (sndbuf_opt < want_sndbuf) {
    int snd_bufsize = message_size * message_count;
    CHECK_ZERO_ERRNO(
        setsockopt(client_sock.get(), SOL_SOCKET, SO_SNDBUF, &snd_bufsize, sizeof(snd_bufsize)));
  }
  CHECK_ZERO_ERRNO(connect(client_sock.get(), sockaddr.as_sockaddr(), sockaddr.socklen()));

  std::vector<uint8_t> send_bytes, recv_bytes;
  // Avoid large memory regions with zeroes that can cause the system to try and reclaim pages from
  // us. For more information see Zircon page scanner and eviction strategies.
  send_bytes.resize(message_size, 0xAA);
  recv_bytes.resize(message_size, 0xBB);

  while (state->KeepRunning()) {
    for (int i = 0; i < message_count; i++) {
      ssize_t wr;
      {
#ifdef __Fuchsia__
        TRACE_DURATION(kSocketBenchmarksTracingCategory, "udp_write");
#endif
        wr = write(client_sock.get(), send_bytes.data(), message_size);
      }
      CHECK_TRUE_ERRNO(wr >= 0);
      FX_CHECK(wr == static_cast<ssize_t>(message_size))
          << "wrote " << wr << " expected " << message_size;
    }
    for (int i = 0; i < message_count; i++) {
      ssize_t rd;
      {
#ifdef __Fuchsia__
        TRACE_DURATION(kSocketBenchmarksTracingCategory, "udp_read");
#endif
        rd = read(server_sock.get(), recv_bytes.data(), message_size);
      }
      CHECK_TRUE_ERRNO(rd >= 0);
      FX_CHECK(rd == static_cast<ssize_t>(message_size))
          << "read " << rd << " expected " << message_size;
    }
  }

  return true;
}

// Tests the ping latency over a loopback socket.
//
// Measures the time to send an echo request over a loopback ICMP socket and observe its response.
template <typename Ip>
bool PingLatency(perftest::RepeatState* state) {
  TemplateIsIpVersion<Ip>();
  using Addr = typename Ip::SockAddr;

  fbl::unique_fd sock;
  CHECK_TRUE_ERRNO(sock = fbl::unique_fd(socket(Ip::kFamily, SOCK_DGRAM, Ip::kIpProtoIcmp)));
  const Addr sockaddr = Ip::loopback();
  CHECK_ZERO_ERRNO(connect(sock.get(), sockaddr.as_sockaddr(), sockaddr.socklen()));

  struct {
    icmphdr icmp;
    char payload[4];
  } send_buffer, recv_buffer;
  uint16_t sequence = 0;
  icmphdr& send_header = send_buffer.icmp;

  while (state->KeepRunning()) {
    send_header = {
        .type = Ip::kIcmpEchoRequestType,
        .un = {.echo = {.sequence = ++sequence}},
    };
    ssize_t wr = write(sock.get(), &send_buffer, sizeof(send_buffer));
    CHECK_TRUE_ERRNO(wr >= 0);
    FX_CHECK(static_cast<size_t>(wr) == sizeof(send_buffer))
        << "wrote " << wr << " expected " << sizeof(send_buffer);

    ssize_t rd = read(sock.get(), &recv_buffer, sizeof(recv_buffer));
    CHECK_TRUE_ERRNO(rd >= 0);
    FX_CHECK(static_cast<size_t>(rd) == sizeof(recv_buffer))
        << "read " << rd << " expected " << sizeof(recv_buffer);
    const icmphdr& header = recv_buffer.icmp;
    FX_CHECK(header.type == Ip::kIcmpEchoReplyType)
        << "received header type " << header.type << ", expected echo response "
        << Ip::kIcmpEchoReplyType;
    FX_CHECK(header.un.echo.sequence == sequence)
        << "received sequence " << header.un.echo.sequence << ", expected sequence " << sequence;
  }

  return true;
}

void RegisterTests() {
  constexpr const char* kSingleReadTestNameFmt = "WriteRead/%s/%s/%ld%s";
  enum class Network { kIpv4, kIpv6 };

  auto network_to_string = [](Network network) {
    switch (network) {
      case Network::kIpv4:
        return "IPv4";
      case Network::kIpv6:
        return "IPv6";
    }
  };

  auto bytes_with_unit = [](size_t bytes) -> std::pair<size_t, const char*> {
    if (bytes >= 1024) {
      bytes /= 1024;
      // Keep "kB" instead of "KiB" to avoid losing benchmarking history.
      return {bytes, "kB"};
    }
    return {bytes, "B"};
  };

  auto get_tcp_test_name = [&bytes_with_unit, &network_to_string](Network network,
                                                                  size_t raw_bytes) -> std::string {
    const char* network_name = network_to_string(network);
    auto [bytes, bytes_unit] = bytes_with_unit(raw_bytes);

    return fxl::StringPrintf(kSingleReadTestNameFmt, "TCP", network_name, bytes, bytes_unit);
  };

  auto get_udp_test_name = [&bytes_with_unit, &network_to_string](
                               // NOLINTNEXTLINE(bugprone-easily-swappable-parameters)
                               Network network, size_t raw_bytes,
                               size_t message_count) -> std::string {
    const char* network_name = network_to_string(network);
    auto [bytes, bytes_unit] = bytes_with_unit(raw_bytes);
    constexpr const char* kUDP = "UDP";
    if (message_count > 1) {
      return fxl::StringPrintf("MultiWriteRead/%s/%s/%ld%s/%ldMessages", kUDP, network_name, bytes,
                               bytes_unit, message_count);
    }
    return fxl::StringPrintf(kSingleReadTestNameFmt, kUDP, network_name, bytes, bytes_unit);
  };

  constexpr int kTransferSizesForTcp[] = {
      1 << 10, 10 << 10, 100 << 10, 500 << 10, 1000 << 10,
  };
  for (int transfer : kTransferSizesForTcp) {
    perftest::RegisterTest(get_tcp_test_name(Network::kIpv4, transfer).c_str(), TcpWriteRead<Ipv4>,
                           transfer);
    perftest::RegisterTest(get_tcp_test_name(Network::kIpv6, transfer).c_str(), TcpWriteRead<Ipv6>,
                           transfer);
  }

  // NB: Knowledge encoded at a distance: these datagrams avoid IP fragmentation
  // only because loopback has a very large MTU.
  constexpr int kMessageSizesForUdp[] = {1, 100, 1 << 10, 10 << 10, 60 << 10};
  // NB: The message count of 50 is approximately as large as possible in
  // conjunction with the 60 KiB message size as the total transfer size is
  // about 3 MB and Netstack 2 enforces a maximum of 4 MiB for socket send/receive
  // buffer sizes.
  constexpr int kMessageCountsForUdp[] = {1, 10, 50};
  for (int message_size : kMessageSizesForUdp) {
    for (int message_count : kMessageCountsForUdp) {
      perftest::RegisterTest(get_udp_test_name(Network::kIpv4, message_size, message_count).c_str(),
                             UdpWriteRead<Ipv4>, message_size, message_count);
      perftest::RegisterTest(get_udp_test_name(Network::kIpv6, message_size, message_count).c_str(),
                             UdpWriteRead<Ipv6>, message_size, message_count);
    }
  }

  auto register_ping = [&network_to_string]() {
    if (!kIsFuchsia) {
      // When running on not-Fuchsia, we may not be permitted to create ICMP sockets.
      if (int fd = socket(AF_INET, SOCK_DGRAM, IPPROTO_ICMP); fd < 0) {
        if (errno == EACCES) {
          std::cout << "ICMP sockets are not permitted; skipping ping benchmarks\n";
          return;
        }
      } else {
        CHECK_ZERO_ERRNO(close(fd));
      }
    }

    constexpr char kPingTestNameFmt[] = "PingLatency/%s";
    perftest::RegisterTest(
        fxl::StringPrintf(kPingTestNameFmt, network_to_string(Network::kIpv4)).c_str(),
        PingLatency<Ipv4>);
    perftest::RegisterTest(
        fxl::StringPrintf(kPingTestNameFmt, network_to_string(Network::kIpv6)).c_str(),
        PingLatency<Ipv6>);
  };
  register_ping();
}

PERFTEST_CTOR(RegisterTests)
}  // namespace

int main(int argc, char** argv) {
  std::string test_suite = "fuchsia.network.socket.loopback";

  if (std::getenv(kStarnixEnvVar)) {
    test_suite += ".starnix";
  }

  if (std::getenv(kFakeNetstackEnvVar)) {
    test_suite += ".fake_netstack";
  } else if (std::getenv(kNetstack3EnvVar)) {
    test_suite += ".netstack3";
  }

#if defined(__Fuchsia__)
  std::optional<Tracer> tracer;
  if (std::getenv(kTracingEnvVar)) {
    const fuchsia_tracing_controller::TraceConfig trace_config{{
        .categories = std::vector<std::string>{"kernel:meta", "kernel:sched", "kernel:syscall",
                                               "net", "perftest", kSocketBenchmarksTracingCategory},
        .buffer_size_megabytes_hint = 64,
    }};
    fit::result<fit::failed, Tracer> result =
        StartTracing(trace_config, "/custom_artifacts/trace.fxt");
    if (result.is_error()) {
      FX_LOGS(ERROR) << "failed to start tracing";
      return 1;
    }
    tracer = std::move(*result);
  }
#endif

  int return_code = perftest::PerfTestMain(argc, argv, test_suite.c_str());

#if defined(__Fuchsia__)
  if (std::getenv(kTracingEnvVar) && tracer.has_value()) {
    fit::result<fit::failed> result = StopTracing(std::move(tracer.value()));
    if (result.is_error()) {
      FX_LOGS(ERROR) << "failed to stop tracing";
      return 1;
    }
  }
#endif

  return return_code;
}
