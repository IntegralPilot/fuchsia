#include <errno.h>
#include <sys/msg.h>

#include "ipc.h"

int msgctl(int q, int cmd, void* buf) {
  errno = ENOSYS;
  return -1;
}
