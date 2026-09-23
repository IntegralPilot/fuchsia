#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

static int check_name(const char* name) {
  while (*name == '/')
    name++;
  if (name[0] == '\0' || !strcmp(name, ".") || !strcmp(name, "..")) {
    errno = EINVAL;
  } else if (strlen(name) > NAME_MAX) {
    errno = ENAMETOOLONG;
  } else {
    errno = ENOENT;
  }
  return -1;
}

int shm_open(const char* name, int flag, mode_t mode) { return check_name(name); }

int shm_unlink(const char* name) { return check_name(name); }
