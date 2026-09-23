// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_OPENTHREAD_THIRD_PARTY_OPENTHREAD_PLATFORM_RADIO_URL_H_
#define SRC_CONNECTIVITY_OPENTHREAD_THIRD_PARTY_OPENTHREAD_PLATFORM_RADIO_URL_H_

#include <stdio.h>
#include <stdlib.h>

#include "openthread-system.h"

#include <url/url.hpp>

namespace ot {
namespace Posix {

/**
 * Implements the radio URL processing.
 *
 */
class RadioUrl : public ot::Url::Url {
 public:
  /**
   * Initializes the object.
   *
   * @param[in]   aUrl    The null-terminated URL string.
   *
   */
  explicit RadioUrl(const char *aUrl) { Init(aUrl); };

  /**
   * Initializes the radio URL.
   *
   * @param[in]   aUrl    The null-terminated URL string.
   *
   */
  void Init(const char *aUrl);

  RadioUrl(const RadioUrl &) = delete;
  RadioUrl(RadioUrl &&) = delete;
  RadioUrl &operator=(const RadioUrl &) = delete;
  RadioUrl &operator=(RadioUrl &&) = delete;

 private:
  enum {
    kRadioUrlMaxSize = 512,
  };
  char mUrl[kRadioUrlMaxSize];
};

}  // namespace Posix
}  // namespace ot

#endif  // SRC_CONNECTIVITY_OPENTHREAD_THIRD_PARTY_OPENTHREAD_PLATFORM_RADIO_URL_H_
