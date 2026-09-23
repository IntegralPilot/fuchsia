#include <unistd.h>
#include <zircon/syscalls.h>

int pause(void) {
  _zx_nanosleep(ZX_TIME_INFINITE);
  __builtin_trap();  // unreachable
}
