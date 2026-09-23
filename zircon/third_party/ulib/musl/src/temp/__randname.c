#include <stdint.h>
#include <time.h>

#include "threads_impl.h"

/* This assumes that a check for the
   template size has already been made */
char* __randname(char* template) {
  int i;
  struct timespec ts;
  unsigned long r;

  __clock_gettime(CLOCK_MONOTONIC, &ts);
  r = ts.tv_nsec + __thread_get_tid() * 65537UL;
  for (i = 0; i < 6; i++, r >>= 5)
    template[i] = (char)('A' + (r & 15) + (r & 16) * 2);

  return template;
}
