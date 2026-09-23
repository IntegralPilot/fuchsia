#define _GNU_SOURCE
#include <errno.h>
#include <sys/time.h>

int adjtime(const struct timeval* in, struct timeval* out) {
  // TODO(kulakowski) implement adjtime(x)
  errno = ENOSYS;
  return -1;
}
