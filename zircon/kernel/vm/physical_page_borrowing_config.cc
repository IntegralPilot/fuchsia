// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-options/boot-options.h>

#include <lk/init.h>
#include <vm/physical_page_borrowing_config.h>

static void ppb_init_func(uint level) {
  // One option per potential borrowing site.

  PhysicalPageBorrowingConfig::Get().set_borrowing_on_mru_enabled(
      BootOptions::Get()->ppb_borrow_on_mru);

  // One option for whether decommit on contiguous VMO can work or returns ZX_ERR_NOT_SUPPORTED.
  PhysicalPageBorrowingConfig::Get().set_loaning_enabled(BootOptions::Get()->ppb_loan);

  PhysicalPageBorrowingConfig::Get().set_replace_on_unloan_enabled(
      BootOptions::Get()->ppb_replace_on_unloan);
}

LK_INIT_HOOK(ppb_init, &ppb_init_func, LK_INIT_LEVEL_VM)
