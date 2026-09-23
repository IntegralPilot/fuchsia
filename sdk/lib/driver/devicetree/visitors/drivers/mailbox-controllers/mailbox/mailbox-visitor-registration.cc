// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/devicetree/visitors/registration.h>

#include "lib/driver/devicetree/visitors/drivers/mailbox-controllers/mailbox/mailbox-visitor.h"

REGISTER_DEVICETREE_VISITOR(mailbox_dt::MailboxVisitor);
