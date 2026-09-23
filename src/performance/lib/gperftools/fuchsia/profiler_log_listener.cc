// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <link.h>

#include <iomanip>
#include <iostream>
#include <sstream>
#include <string>
#include <vector>

class MapsDumper {
  public:
   MapsDumper() : os_(&buf_) {}

   int Iterate(struct dl_phdr_info* module, size_t) {
     for (ElfW(Half) i = 0; i < module->dlpi_phnum; i++) {
       const ElfW(Phdr)* entry = &module->dlpi_phdr[i];
       os_ << std::hex << std::setfill('0') << std::setw(12) << (module->dlpi_addr + entry->p_vaddr)
           << '-' << std::setw(12) << (module->dlpi_addr + entry->p_vaddr + entry->p_memsz)
           << " " << (entry->p_flags & PF_R ? 'r' : '-') << (entry->p_flags & PF_W ? 'w' : '-') << (entry->p_flags & PF_X ? 'x' : '-') << "p "
           << std::setw(8) << std::setfill('0') << entry->p_offset << " " << "00:00"
           << std::setw(4) << std::setfill(' ') << std::dec << 0
           << " " << (strcmp(module->dlpi_name, "") == 0 ? "<application>" : module->dlpi_name) << '\n';
     }
     return 0;
   }

   std::string Dump() {
     return buf_.str();
   }

  private:
   std::ostream os_;
   std::stringbuf buf_;
};

std::string CollectProfilerLog() {
  MapsDumper dumper;
  dl_iterate_phdr([](struct dl_phdr_info* module, size_t aa, void *self) {
    return reinterpret_cast<MapsDumper*>(self)->Iterate(module, aa);
  }, &dumper);
  return dumper.Dump();
}
