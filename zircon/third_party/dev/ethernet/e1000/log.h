// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#ifndef ZIRCON_THIRD_PARTY_DEV_ETHERNET_E1000_LOG_H_
#define ZIRCON_THIRD_PARTY_DEV_ETHERNET_E1000_LOG_H_

#include <sdk/lib/syslog/structured_backend/fuchsia_syslog.h>
#include <zircon/compiler.h>

__BEGIN_CDECLS

// Because this file is included from C-code it needs to wrap calls to FDF_LOG as they only work
// with C++.
void e1000_logf(FuchsiaLogSeverity severity, const char* file, int line, const char* msg, ...)
    __PRINTFLIKE(4, 5);

#define E1000_LOGF(severity, msg...) e1000_logf(FUCHSIA_LOG_##severity, __FILE__, __LINE__, msg)

__END_CDECLS

#endif  // ZIRCON_THIRD_PARTY_DEV_ETHERNET_E1000_LOG_H_
