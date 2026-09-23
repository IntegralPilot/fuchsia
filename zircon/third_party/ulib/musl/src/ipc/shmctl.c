#include <errno.h>
#include <sys/shm.h>

#include "ipc.h"

int shmctl(int id, int cmd, void* buf) {
  errno = ENOSYS;
  return -1;
}
