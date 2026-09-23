// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/annotations/startup_annotations.h"

#include <lib/syslog/cpp/macros.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/feedback/annotations/constants.h"
#include "src/developer/forensics/feedback/constants.h"
#include "src/developer/forensics/feedback/reboot_log/final_shutdown_info.h"
#include "src/developer/forensics/testing/gmatchers.h"
#include "src/developer/forensics/testing/gpretty_printers.h"  // IWYU pragma: keep
#include "src/developer/forensics/testing/scoped_memfs_manager.h"
#include "src/lib/files/file.h"

namespace forensics::feedback {
namespace {

using ::testing::_;
using ::testing::Contains;
using ::testing::Key;
using ::testing::Pair;
using ::testing::UnorderedElementsAre;
using ::testing::UnorderedElementsAreArray;

constexpr char kTestCompilationModePath[] = "/tmp/build_compilation_mode.txt";

class StartupAnnotationsTest : public ::testing::Test {
 public:
  void SetUp() override {}

  void TearDown() override {}

  void WriteFile(const std::string& path, const std::string& data) {
    FX_CHECK(files::WriteFile(path, data)) << "Failed to write to " << path;
  }

  void WriteFiles(const std::map<std::string, std::string>& paths_and_data) {
    for (const auto& [path, data] : paths_and_data) {
      WriteFile(path, data);
    }
  }
};

TEST_F(StartupAnnotationsTest, Keys) {
  const FinalShutdownInfo final_shutdown_info(FinalShutdownReason::kOom,
                                              GracefulShutdownAction::kReboot);
  const Annotations startup_annotations =
      GetStartupAnnotations(final_shutdown_info, SpontaneousRebootReason::kSpontaneous,
                            /*compilation_mode_path=*/kTestCompilationModePath);

  EXPECT_THAT(startup_annotations, UnorderedElementsAreArray({
                                       Key(kBuildBoardKey),
                                       Key(kBuildCompilationModeKey),
                                       Key(kBuildProductKey),
                                       Key(kBuildLatestCommitDateKey),
                                       Key(kBuildPlatformBackstopKey),
                                       Key(kBuildVersionKey),
                                       Key(kBuildVersionPreviousBootKey),
                                       Key(kBuildPlatformVersionKey),
                                       Key(kBuildPlatformVersionPreviousBootKey),
                                       Key(kBuildProductVersionKey),
                                       Key(kBuildProductVersionPreviousBootKey),
                                       Key(kDeviceBoardNameKey),
                                       Key(kDeviceNumCPUsKey),
                                       Key(kSystemBootIdCurrentKey),
                                       Key(kSystemBootIdTimelineKey),
                                       Key(kSystemBootIdPreviousKey),
                                       Key(kSystemLastRebootReasonKey),
                                       Key(kSystemLastRebootRuntimeKey),
                                       Key(kSystemLastRebootTotalSuspendedTimeKey),
                                       Key(kSystemLastRebootUptimeKey),
                                       Key(kSystemLastShutdownGracefulActionKey),
                                   }));
}

TEST_F(StartupAnnotationsTest, Values_FilesPresent) {
  testing::ScopedMemFsManager memfs_manager;

  memfs_manager.Create("/boot/config/build_info");
  memfs_manager.Create("/config/build-info");
  memfs_manager.Create("/cache");
  memfs_manager.Create("/data");
  memfs_manager.Create("/tmp");

  WriteFiles({
      {kBuildBoardPath, "board"},
      {kBuildProductPath, "product"},
      {kBuildCommitDatePath, "commit-date"},
      {kBuildMinUtcStampPath, "1748946819"},
      {kTestCompilationModePath, "compilation-mode"},
      {kCurrentBuildVersionPath, "current-version"},
      {kPreviousBuildVersionPath, "previous-version"},
      {kCurrentBuildPlatformVersionPath, "current-platform-version"},
      {kPreviousBuildPlatformVersionPath, "previous-platform-version"},
      {kCurrentBuildProductVersionPath, "current-product-version"},
      {kPreviousBuildProductVersionPath, "previous-product-version"},
      {kCurrentBootIdPath, "current-boot-id"},
      {kPreviousBootIdPath, "previous-boot-id"},
      {kBootIdTimelinePath, "boot-id-timeline"},
  });

  const FinalShutdownInfo final_shutdown_info(FinalShutdownReason::kOom,
                                              GracefulShutdownAction::kReboot);
  const Annotations startup_annotations =
      GetStartupAnnotations(final_shutdown_info, SpontaneousRebootReason::kSpontaneous,
                            /*compilation_mode_path=*/kTestCompilationModePath);

  EXPECT_THAT(
      startup_annotations,
      UnorderedElementsAre(
          Pair(kBuildBoardKey, ErrorOrString("board")),
          Pair(kBuildCompilationModeKey, ErrorOrString("compilation-mode")),
          Pair(kBuildProductKey, ErrorOrString("product")),
          Pair(kBuildLatestCommitDateKey, ErrorOrString("commit-date")),
          Pair(kBuildPlatformBackstopKey, ErrorOrString("2025-06-03T10:33:39+00:00")),
          Pair(kBuildVersionKey, ErrorOrString("current-version")),
          Pair(kBuildVersionPreviousBootKey, ErrorOrString("previous-version")),
          Pair(kBuildPlatformVersionKey, ErrorOrString("current-platform-version")),
          Pair(kBuildPlatformVersionPreviousBootKey, ErrorOrString("previous-platform-version")),
          Pair(kBuildProductVersionKey, ErrorOrString("current-product-version")),
          Pair(kBuildProductVersionPreviousBootKey, ErrorOrString("previous-product-version")),
          Pair(kDeviceBoardNameKey, _), Pair(kDeviceNumCPUsKey, _),
          Pair(kSystemBootIdCurrentKey, ErrorOrString("current-boot-id")),
          Pair(kSystemBootIdPreviousKey, ErrorOrString("previous-boot-id")),
          Pair(kSystemBootIdTimelineKey, ErrorOrString("boot-id-timeline")),
          Pair(kSystemLastRebootReasonKey,
               ErrorOrString(final_shutdown_info.ToSnapshotAnnotationReason(
                   SpontaneousRebootReason::kSpontaneous))),
          Pair(kSystemLastRebootRuntimeKey, final_shutdown_info.ToSnapshotAnnotationRuntime()),
          Pair(kSystemLastRebootTotalSuspendedTimeKey,
               final_shutdown_info.ToSnapshotAnnotationTotalSuspendedTime()),
          Pair(kSystemLastRebootUptimeKey, final_shutdown_info.ToSnapshotAnnotationUptime()),
          Pair(kSystemLastShutdownGracefulActionKey,
               final_shutdown_info.ToSnapshotAnnotationGracefulAction())));
}

TEST_F(StartupAnnotationsTest, Values_FilesMissing) {
  const FinalShutdownInfo final_shutdown_info(FinalShutdownReason::kOom,
                                              GracefulShutdownAction::kReboot);
  const Annotations startup_annotations =
      GetStartupAnnotations(final_shutdown_info, SpontaneousRebootReason::kSpontaneous,
                            /*compilation_mode_path=*/kTestCompilationModePath);

  EXPECT_THAT(
      startup_annotations,
      UnorderedElementsAre(
          Pair(kBuildBoardKey, ErrorOrString(Error::kFileReadFailure)),
          Pair(kBuildCompilationModeKey, ErrorOrString(Error::kFileReadFailure)),
          Pair(kBuildProductKey, ErrorOrString(Error::kFileReadFailure)),
          Pair(kBuildLatestCommitDateKey, ErrorOrString(Error::kFileReadFailure)),
          Pair(kBuildPlatformBackstopKey, ErrorOrString(Error::kFileReadFailure)),
          Pair(kBuildVersionKey, ErrorOrString(Error::kFileReadFailure)),
          Pair(kBuildVersionPreviousBootKey, ErrorOrString(Error::kFileReadFailure)),
          Pair(kBuildPlatformVersionKey, ErrorOrString(Error::kFileReadFailure)),
          Pair(kBuildPlatformVersionPreviousBootKey, ErrorOrString(Error::kFileReadFailure)),
          Pair(kBuildProductVersionKey, ErrorOrString(Error::kFileReadFailure)),
          Pair(kBuildProductVersionPreviousBootKey, ErrorOrString(Error::kFileReadFailure)),
          Pair(kDeviceBoardNameKey, _), Pair(kDeviceNumCPUsKey, _),
          Pair(kSystemBootIdCurrentKey, ErrorOrString(Error::kFileReadFailure)),
          Pair(kSystemBootIdPreviousKey, ErrorOrString(Error::kFileReadFailure)),
          Pair(kSystemBootIdTimelineKey, ErrorOrString(Error::kFileReadFailure)),
          Pair(kSystemLastRebootReasonKey,
               ErrorOrString(final_shutdown_info.ToSnapshotAnnotationReason(
                   SpontaneousRebootReason::kSpontaneous))),
          Pair(kSystemLastRebootRuntimeKey, final_shutdown_info.ToSnapshotAnnotationRuntime()),
          Pair(kSystemLastRebootTotalSuspendedTimeKey,
               final_shutdown_info.ToSnapshotAnnotationTotalSuspendedTime()),
          Pair(kSystemLastRebootUptimeKey, final_shutdown_info.ToSnapshotAnnotationUptime()),
          Pair(kSystemLastShutdownGracefulActionKey,
               final_shutdown_info.ToSnapshotAnnotationGracefulAction())));
}

TEST_F(StartupAnnotationsTest, BackstopTime_Invalid) {
  testing::ScopedMemFsManager memfs_manager;

  memfs_manager.Create("/boot/config/build_info");

  WriteFiles({
      {kBuildMinUtcStampPath, "invalid"},
  });

  const FinalShutdownInfo final_shutdown_info(FinalShutdownReason::kOom,
                                              GracefulShutdownAction::kReboot);
  const Annotations startup_annotations =
      GetStartupAnnotations(final_shutdown_info, SpontaneousRebootReason::kSpontaneous,
                            /*compilation_mode_path=*/kTestCompilationModePath);

  EXPECT_THAT(startup_annotations,
              Contains(Pair(kBuildPlatformBackstopKey, ErrorOrString(Error::kBadValue))));
}

TEST_F(StartupAnnotationsTest, BuildProductVersionPreviousBootFallback) {
  testing::ScopedMemFsManager memfs_manager;
  memfs_manager.Create("/data");
  memfs_manager.Create("/tmp");

  // On the first OTA, the build platform and product versions for the previous boot won't be
  // available. The build product version should match the legacy build version.
  WriteFiles({
      {kCurrentBuildVersionPath, "current-version"},
      {kPreviousBuildVersionPath, "previous-version"},
      {kCurrentBuildPlatformVersionPath, "current-platform-version"},
      {kCurrentBuildProductVersionPath, "current-product-version"},
  });

  const FinalShutdownInfo final_shutdown_info(FinalShutdownReason::kOom,
                                              GracefulShutdownAction::kReboot);
  const Annotations startup_annotations =
      GetStartupAnnotations(final_shutdown_info, SpontaneousRebootReason::kSpontaneous,
                            /*compilation_mode_path=*/kTestCompilationModePath);

  EXPECT_THAT(
      startup_annotations,
      UnorderedElementsAre(
          Pair(kBuildBoardKey, _), Pair(kBuildCompilationModeKey, _), Pair(kBuildProductKey, _),
          Pair(kBuildLatestCommitDateKey, _), Pair(kBuildPlatformBackstopKey, _),
          Pair(kBuildVersionKey, ErrorOrString("current-version")),
          Pair(kBuildVersionPreviousBootKey, ErrorOrString("previous-version")),
          Pair(kBuildPlatformVersionKey, ErrorOrString("current-platform-version")),
          Pair(kBuildPlatformVersionPreviousBootKey, ErrorOrString(Error::kFileReadFailure)),
          Pair(kBuildProductVersionKey, ErrorOrString("current-product-version")),
          Pair(kBuildProductVersionPreviousBootKey, ErrorOrString("previous-version")),
          Pair(kDeviceBoardNameKey, _), Pair(kDeviceNumCPUsKey, _),
          Pair(kSystemBootIdCurrentKey, _), Pair(kSystemBootIdPreviousKey, _),
          Pair(kSystemBootIdTimelineKey, _), Pair(kSystemLastRebootRuntimeKey, _),
          Pair(kSystemLastRebootTotalSuspendedTimeKey, _), Pair(kSystemLastRebootUptimeKey, _),
          Pair(kSystemLastShutdownGracefulActionKey, _), Pair(kSystemLastRebootReasonKey, _)));
}

}  // namespace
}  // namespace forensics::feedback
