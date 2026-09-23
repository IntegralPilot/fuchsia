// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_GIGABOOT_CPP_FASTBOOT_H_
#define SRC_FIRMWARE_GIGABOOT_CPP_FASTBOOT_H_

#include <lib/abr/ops.h>
#include <lib/fastboot/fastboot_base.h>
#include <lib/zircon_boot/zircon_boot.h>
#include <stdarg.h>

#include <functional>
#include <memory>
#include <span>
#include <string_view>
#include <variant>

#include "backends.h"
#include "efi_variables.h"
#include "utils.h"

namespace gigaboot {

[[gnu::format(printf, 2, 3)]] void hexdump_printer_printf(void *printf_arg, const char *fmt, ...);

class Fastboot : public fastboot::FastbootBase {
 public:
  Fastboot(std::span<uint8_t> download_buffer, ZirconBootOps zb_ops,
           EfiVariables *efi_variables = nullptr)
      : download_buffer_(download_buffer), zb_ops_(zb_ops), efi_variables_(efi_variables) {
    if (!efi_variables_) {
      owned_efi_variables_ = std::make_unique<EfiVariables>();
      efi_variables_ = owned_efi_variables_.get();
    }
  }
  bool IsContinue() { return continue_; }

  // 'vprintf' signature for output testing.
  using VPrintFunction = std::function<int(const char *fmt, va_list ap)>;
  void SetVPrintFunction(VPrintFunction vprint_function) { vprinter_ = vprint_function; }

 private:
  zx::result<> ProcessCommand(std::string_view cmd, fastboot::Transport *transport) override;
  void DoClearDownload() override;
  zx::result<void *> GetDownloadBuffer(size_t total_download_size) override;
  AbrOps GetAbrOps() { return GetAbrOpsFromZirconBootOps(&zb_ops_); }

  // For getvar:all, we need to customize SendResponse to
  // 1) send INFO instead of OKAY messages
  // 2) prepend the name of the variable for each variable and optionally its argument
  //
  // The way this is implemented is by capturing the variable name,
  // which means we need a type that abstracts over function pointers and callable objects.
  using Responder =
      std::function<zx::result<>(ResponseType, std::string_view, fastboot::Transport *)>;

  // Minor hack to call SendResponse with the default value of zx::ok()
  static constexpr auto DefaultResponder = [](ResponseType r, std::string_view s,
                                              fastboot::Transport *t) {
    return SendResponse(r, s, t);
  };

  // A function to call to determine the value of a variable.
  // Variables with constant, i.e. compile-time values should instead
  // define their value via the string variant.
  using VarFunc = zx::result<> (Fastboot::*)(const CommandArgs &, fastboot::Transport *,
                                             const Responder &);
  struct VarFuncAndArgs {
    VarFunc func;
    std::span<const std::string_view> arg_list = {};
  };

  struct VariableEntry {
    std::string_view name;
    std::variant<VarFuncAndArgs, std::string_view> var;
  };

  std::span<VariableEntry> GetVariableTable();

  struct CommandCallbackEntry {
    std::string_view name;
    zx::result<> (Fastboot::*cmd)(std::string_view, fastboot::Transport *);
  };

  std::span<CommandCallbackEntry> GetCommandCallbackTable();

  zx::result<> GetVarAll(const CommandArgs &, fastboot::Transport *, const Responder &);
  zx::result<> GetVarMaxDownloadSize(const CommandArgs &, fastboot::Transport *, const Responder &);
  zx::result<> GetVarCurrentSlot(const CommandArgs &, fastboot::Transport *, const Responder &);
  zx::result<> GetVarSlotLastSetActive(const CommandArgs &, fastboot::Transport *,
                                       const Responder &);
  zx::result<> GetVarSlotRetryCount(const CommandArgs &, fastboot::Transport *, const Responder &);
  zx::result<> GetVarSlotSuccessful(const CommandArgs &, fastboot::Transport *, const Responder &);
  zx::result<> GetVarSlotUnbootable(const CommandArgs &, fastboot::Transport *, const Responder &);
  zx::result<> GetVarSlotUnbootableReason(const CommandArgs &, fastboot::Transport *,
                                          const Responder &);

  zx::result<> GetVar(std::string_view cmd, fastboot::Transport *transport);
  zx::result<> Flash(std::string_view cmd, fastboot::Transport *transport);
  // `fastboot boot`: boot the downloaded ZBI directly out of RAM, writing
  // nothing to storage. This is the standard Android development loop.
  zx::result<> BootRam(std::string_view cmd, fastboot::Transport *transport);
  zx::result<> Continue(std::string_view cmd, fastboot::Transport *transport);
  zx::result<> DoReboot(RebootMode reboot_mode, std::string_view cmd,
                        fastboot::Transport *transport);
  zx::result<> Reboot(std::string_view cmd, fastboot::Transport *transport);
  zx::result<> RebootBootloader(std::string_view cmd, fastboot::Transport *transport);
  zx::result<> RebootRecovery(std::string_view cmd, fastboot::Transport *transport);
  zx::result<> SetActive(std::string_view cmd, fastboot::Transport *transport);
  zx::result<> OemAddStagedBootloaderFile(std::string_view cmd, fastboot::Transport *transport);

  zx::result<> EfiGetVarInfo(std::string_view cmd, fastboot::Transport *transport);
  zx::result<> EfiGetVarNames(std::string_view cmd, fastboot::Transport *transport);
  zx::result<> EfiGetVar(std::string_view cmd, fastboot::Transport *transport);
  zx::result<> EfiDumpVars(std::string_view cmd, fastboot::Transport *transport);

  // OEM commands
  zx::result<> GptInit(std::string_view cmd, fastboot::Transport *transport);

  class TransportScope {
    Fastboot &owner;

    TransportScope() = delete;
    TransportScope(const TransportScope &) = delete;
    TransportScope &operator=(const TransportScope &) = delete;

   public:
    explicit TransportScope(Fastboot &owner_in) : owner(owner_in) {}
    ~TransportScope() { owner.UnRegisterTransport(); }
  };
  void UnRegisterTransport() { print_transport_ = nullptr; }

  // Set current transport to use for sending text via kInfo from `vprinter()`
  // Returns `TransportScope` object that calls `UnRegisterTransport()` on dustruction to stop using
  // `transport` when it is out of scope.
  TransportScope RegisterTransport(fastboot::Transport *transport) {
    print_transport_ = transport;
    return TransportScope(*this);
  }

  // If set, this transport is used to send text from `vprinter_()` via kInfo channel.
  fastboot::Transport *print_transport_ = nullptr;

  [[gnu::format(printf, 3, 0)]]
  void InfoSend(fastboot::Transport *transport, const char *fmt, va_list va);

  // Print function for testing and sending text via kInfo channel
  VPrintFunction vprinter_ = [this] [[gnu::format(printf, 2, 0)]] (const char *fmt, va_list ap) {
    if (print_transport_) {
      va_list ap2;
      va_copy(ap2, ap);
      InfoSend(print_transport_, fmt, ap2);
      va_end(ap2);
    }
    return vprintf(fmt, ap);
  };
  [[gnu::format(printf, 2, 3)]] int printer_(const char *fmt, ...);
  friend void hexdump_printer_printf(void *printf_arg, const char *fmt, ...);

  std::span<uint8_t> download_buffer_;
  ZirconBootOps zb_ops_;
  bool continue_ = false;

  std::unique_ptr<EfiVariables> owned_efi_variables_;
  EfiVariables *efi_variables_;
};

// APIs for fastboot over tcp.

class TcpTransportInterface {
 public:
  // Interface for reading from/writing to a tcp connection. Implementation should
  // guarantee that these operations are blocking.
  virtual bool Read(void *out, size_t size) = 0;
  virtual bool Write(const void *data, size_t size) = 0;
};

constexpr size_t kFastbootHandshakeMessageLength = 4;
constexpr size_t kFastbootTcpLengthPrefixBytes = 8;

// Run a fastboot session after tcp connection is established.
void FastbootTcpSession(TcpTransportInterface &interface, Fastboot &fastboot);

}  // namespace gigaboot

#endif  // SRC_FIRMWARE_GIGABOOT_CPP_FASTBOOT_H_
