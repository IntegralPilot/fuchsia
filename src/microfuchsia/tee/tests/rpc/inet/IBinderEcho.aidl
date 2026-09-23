// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

interface IBinderEcho {
  @utf8InCpp String echo(@utf8InCpp String str);
}
