// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.examples.services/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>

#include <cstdlib>
#include <utility>

struct Account {
  // Account owner's name
  std::string name;
  // Account balance in cents
  int64_t balance;
};

// Implementation of the `fuchsia.examples.services/ReadOnlyAccount` protocol
class ProtectedAccount : public fidl::Server<fuchsia_examples_services::ReadOnlyAccount> {
 public:
  explicit ProtectedAccount(Account account) : account_(std::move(account)) {}

  void GetOwner(GetOwnerCompleter::Sync& completer) override { completer.Reply({account_.name}); }
  void GetBalance(GetBalanceCompleter::Sync& completer) override {
    completer.Reply({account_.balance});
  }

 private:
  Account account_;
};

// Implementation of the `fuchsia.examples.services/ReadWriteAccount` protocol
class OpenAccount : public fidl::Server<fuchsia_examples_services::ReadWriteAccount> {
 public:
  explicit OpenAccount(Account account) : account_(std::move(account)) {}

  void GetOwner(GetOwnerCompleter::Sync& completer) override { completer.Reply({account_.name}); }

  void GetBalance(GetBalanceCompleter::Sync& completer) override {
    completer.Reply({account_.balance});
  }

  void Debit(DebitRequest& request, DebitCompleter::Sync& completer) override {
    if (account_.balance >= request.amount()) {
      account_.balance -= request.amount();
      completer.Reply({true});
    } else {
      completer.Reply({false});
    }
    FX_LOG_KV(INFO, "Account balance updated: ", FX_KV("balance", account_.balance));
  }

  void Credit(CreditRequest& request, CreditCompleter::Sync& completer) override {
    account_.balance += request.amount();
    completer.Reply();
    FX_LOG_KV(INFO, "Account balance updated: ", FX_KV("balance", account_.balance));
  }

 private:
  Account account_;
};

int main(int argc, const char* argv[], char* envp[]) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  // Read program arguments and construct the account
  if (argc < 3) {
    FX_LOG_KV(ERROR, "Invalid number of arguments.");
    return -1;
  }

  component::OutgoingDirectory outgoing(loop.dispatcher());
  auto result = outgoing.ServeFromStartupInfo();
  if (result.is_error()) {
    FX_LOG_KV(ERROR, "Failed to serve outgoing directory.");
    return -1;
  }

  auto name = argv[1];
  auto balance = atoi(argv[2]);
  Account user_account = {.name = name, .balance = balance};
  FX_LOG_KV(INFO, "Starting bank account provider", FX_KV("name", user_account.name.c_str()),
            FX_KV("balance", user_account.balance));

  fidl::ServerBindingGroup<fuchsia_examples_services::ReadOnlyAccount> read_only_bindings;
  fidl::ServerBindingGroup<fuchsia_examples_services::ReadWriteAccount> read_write_bindings;

  ProtectedAccount protected_account(user_account);
  OpenAccount open_account(user_account);

  auto handler = fuchsia_examples_services::BankAccount::InstanceHandler({
      .read_only = read_only_bindings.CreateHandler(&protected_account, loop.dispatcher(),
                                                    fidl::kIgnoreBindingClosure),
      .read_write = read_write_bindings.CreateHandler(&open_account, loop.dispatcher(),
                                                      fidl::kIgnoreBindingClosure),
  });

  result = outgoing.AddService<fuchsia_examples_services::BankAccount>(std::move(handler));
  if (result.is_error()) {
    FX_LOG_KV(ERROR, "Failed to add service to outgoing.");
    return -1;
  }

  loop.Run();
  return 0;
}
