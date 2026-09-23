#define _GNU_SOURCE
#include <unistd.h>
#include <zircon/syscalls.h>

#include "libc.h"

int getpagesize(void) { return (int)_zx_system_get_page_size(); }
