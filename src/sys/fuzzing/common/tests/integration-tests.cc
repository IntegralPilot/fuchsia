// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sys/fuzzing/common/tests/integration-tests.h"

#include "src/sys/fuzzing/common/artifact.h"
#include "src/sys/fuzzing/common/async-socket.h"
#include "src/sys/fuzzing/common/engine.h"
#include "src/sys/fuzzing/common/testing/artifact.h"
#include "src/sys/fuzzing/common/testing/component-context.h"

namespace fuzzing {

// Test fixtures.

void EngineIntegrationTest::SetUp() {
  AsyncTest::SetUp();
  context_ = ComponentContextForTest::Create(executor());
  engine_ = std::make_unique<ChildProcess>(executor());
  options_ = MakeOptions();
}

void EngineIntegrationTest::AddArg(std::string_view arg) { cmdline_.emplace_back(arg); }

ZxPromise<> EngineIntegrationTest::StartEngine() {
  registrar_ = std::make_unique<FakeRegistrar>(executor());
  return fpromise::make_promise([this]() -> ZxResult<> {
           fidl::InterfaceHandle<Registrar> registrar = registrar_->NewBinding();
           engine_->Reset();
           for (const auto& arg : cmdline_) {
             if (auto status = engine_->AddArg(arg); status != ZX_OK) {
               return fpromise::error(status);
             }
           }
           if (auto status = engine_->AddChannel(ComponentContextForTest::kRegistrarId,
                                                 registrar.TakeChannel());
               status != ZX_OK) {
             return fpromise::error(status);
           }
           return fpromise::ok();
         })
      .and_then([this]() { return AsZxResult(engine_->Spawn()); })
      .wrap_with(scope_);
}

ZxPromise<> EngineIntegrationTest::Connect() {
  FX_DCHECK(registrar_);
  Bridge<> bridge1;
  ZxBridge<> bridge2;
  return registrar_->TakeProvider()
      .then([this, completer = std::move(bridge1.completer)](
                Result<ControllerProviderHandle>& result) mutable -> ZxResult<> {
        if (result.is_error()) {
          FX_LOGS(ERROR) << "Failed to get handle for the controller provider.";
          return fpromise::error(ZX_ERR_INTERNAL);
        }
        auto handle = result.take_value();
        provider_ = handle.Bind();
        provider_->Connect(controller_.NewRequest(executor()->dispatcher()), completer.bind());
        return fpromise::ok();
      })
      .and_then(ConsumeBridge(bridge1))
      .and_then([this, completer = std::move(bridge2.completer)]() mutable -> ZxResult<> {
        controller_->Configure(CopyOptions(*options_), ZxBind<>(std::move(completer)));
        return fpromise::ok();
      })
      .and_then(ConsumeBridge(bridge2))
      .and_then([this]() {
        // Get the controller's current artifact, which should be empty.
        return WatchArtifact(executor(), controller_);
      })
      .and_then([](Artifact& artifact) -> ZxResult<> {
        if (!artifact.is_empty()) {
          FX_LOGS(ERROR) << "Artifact is not empty upon connection: fuzz_result="
                         << artifact.fuzz_result();
          return fpromise::error(ZX_ERR_BAD_STATE);
        }
        return fpromise::ok();
      })
      .wrap_with(scope_);
}

ZxPromise<> EngineIntegrationTest::GetArtifactAndStatus(Artifact* out_artifact,
                                                        Status* out_status) {
  Bridge<Status> bridge;
  // Wait for an update to the controller's artifact. For this to be a "hanging get", a previous
  // call to `WatchArtifact` is needed to get the "baseline" value; see `Connect` above.
  return WatchArtifact(executor(), controller_)
      .and_then([this, out_artifact, completer = std::move(bridge.completer)](
                    Artifact& artifact) mutable -> ZxResult<> {
        *out_artifact = std::move(artifact);
        controller_->GetStatus(completer.bind());
        return fpromise::ok();
      })
      .and_then(ConsumeBridge(bridge))
      .and_then([out_status](Status& status) mutable -> ZxResult<> {
        *out_status = std::move(status);
        return fpromise::ok();
      })
      .wrap_with(scope_);
}

ZxPromise<int64_t> EngineIntegrationTest::WaitForEngine() { return engine_->Wait(); }

void EngineIntegrationTest::TearDown() {
  Schedule(engine_->Kill());
  RunUntilIdle();
  AsyncTest::TearDown();
}

// Integration tests.

void EngineIntegrationTest::RunBounded() {
  options()->set_runs(100);
  options()->set_max_input_size(3);

  FUZZING_EXPECT_OK(StartEngine().and_then(Connect()));
  RunUntilIdle();

  ZxBridge<> bridge;
  controller()->Fuzz(ZxBind<>(std::move(bridge.completer)));
  FUZZING_EXPECT_OK(ConsumeBridge(bridge));
  RunUntilIdle();

  Artifact artifact;
  Status status;
  FUZZING_EXPECT_OK(GetArtifactAndStatus(&artifact, &status));
  RunUntilIdle();

  EXPECT_EQ(artifact.fuzz_result(), FuzzResult::NO_ERRORS);
  EXPECT_FALSE(artifact.has_input());

  // The use of a seed corpus and an input pipelining means we may go over the
  // number of runs by a bit.
  ASSERT_TRUE(status.has_runs());
  EXPECT_GE(status.runs(), 100U);
}

void EngineIntegrationTest::TryCrashingInput() {
  FUZZING_EXPECT_OK(StartEngine().and_then(Connect()));
  RunUntilIdle();

  Input input("FUZZ");
  ZxBridge<> bridge;
  controller()->TryOne(AsyncSocketWrite(executor(), input), ZxBind<>(std::move(bridge.completer)));
  FUZZING_EXPECT_OK(ConsumeBridge(bridge));
  RunUntilIdle();

  Artifact artifact;
  Status status;
  FUZZING_EXPECT_OK(GetArtifactAndStatus(&artifact, &status));
  RunUntilIdle();

  EXPECT_EQ(artifact.fuzz_result(), FuzzResult::CRASH);
  EXPECT_TRUE(status.has_elapsed());
}

void EngineIntegrationTest::RunAsTest() {
  // When run as a test, the fuzzer does not have a controller to connect to.
  int64_t exit_code = -1;
  FUZZING_EXPECT_OK(StartEngine().and_then(WaitForEngine()), &exit_code);
  RunUntilIdle();

  EXPECT_EQ(exit_code, 0);
}

}  // namespace fuzzing
