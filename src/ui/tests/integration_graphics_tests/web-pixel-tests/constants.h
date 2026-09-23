// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_TESTS_INTEGRATION_GRAPHICS_TESTS_WEB_PIXEL_TESTS_CONSTANTS_H_
#define SRC_UI_TESTS_INTEGRATION_GRAPHICS_TESTS_WEB_PIXEL_TESTS_CONSTANTS_H_

// Name of the files containing the HTML code to be used in web_runner_pixeltest. These constants
// are shared across web_runner_pixeltest and http-server.
constexpr auto kStaticHtml = "static.html";
constexpr auto kDynamicHtml = "dynamic.html";
constexpr auto kVideoHtml = "video.html";

// Video file used in |kVideoHtml|.
constexpr auto kFourColorsVideo = "four-colors.webm";

// Port on which the Http server listens to incoming requests. An obscure port is chosen
// purposefully so that |bind(...)| does not fail because the port is already being used by some
// other process.
constexpr int kPort = 81;

#endif  // SRC_UI_TESTS_INTEGRATION_GRAPHICS_TESTS_WEB_PIXEL_TESTS_CONSTANTS_H_
