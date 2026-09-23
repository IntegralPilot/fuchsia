// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "environment_status.h"

#include <cstdlib>
#include <cstring>

namespace analytics::core_dev_tools {

namespace {

constexpr BotInfo kBotEnvironments[] = {{"TEST_ONLY_ENV", "test-only"},
                                        {"TF_BUILD", "azure"},
                                        {"bamboo.buildKey", "bamboo"},
                                        {"BUILDKITE", "buildkite"},
                                        {"CIRCLECI", "circle"},
                                        {"CIRRUS_CI", "cirrus"},
                                        {"CODEBUILD_BUILD_ID", "codebuild"},
                                        {"UNITTEST_ON_BORG", "borg"},
                                        {"UNITTEST_ON_FORGE", "forge"},
                                        {"SWARMING_BOT_ID", "luci"},
                                        {"GITHUB_ACTIONS", "github"},
                                        {"GITLAB_CI", "gitlab"},
                                        {"HEROKU_TEST_RUN_ID", "heroku"},
                                        {"BUILD_ID", "hudson-jenkins"},
                                        {"TEAMCITY_VERSION", "teamcity"},
                                        {"TRAVIS", "travis"},
                                        {"BUILD_NUMBER", "android-ci"}};

// Some bot environment does not set special environment variables but uses "builder" as the user
// name.
// TODO(fxr/126204): Remove this when better alternative is available
constexpr BotInfo kBotBuilder{"USER", "builder"};

}  // namespace

bool IsRunByBot() { return GetBotInfo().IsRunByBot(); }

BotInfo GetBotInfo() {
  for (const auto& bot : kBotEnvironments) {
    if (std::getenv(bot.environment)) {
      return bot;
    }
  }

  // TODO(fxr/126204): Remove this logic when better alternative is available
  const char* user = std::getenv(kBotBuilder.environment);
  if (user && strcmp(user, kBotBuilder.name) == 0) {
    return kBotBuilder;
  }

  return {};
}

bool BotInfo::IsRunByBot() const { return environment != nullptr; }

bool IsDisabledByEnvironment() { return std::getenv("FUCHSIA_ANALYTICS_DISABLED") != nullptr; }

}  // namespace analytics::core_dev_tools
