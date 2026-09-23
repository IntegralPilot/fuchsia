#include "threads_impl.h"

int pthread_attr_getschedparam(const pthread_attr_t* restrict a,
                               struct sched_param* restrict param) {
  param->sched_priority = a->_a_prio;
  return 0;
}

int pthread_condattr_getclock(const pthread_condattr_t* restrict a, clockid_t* restrict clk) {
  *clk = a->__attr & 0x7fffffff;
  return 0;
}

int pthread_mutexattr_getprotocol(const pthread_mutexattr_t* restrict a, int* restrict protocol) {
  *protocol = (a->__attr >> PTHREAD_MUTEX_PROTOCOL_SHIFT) & PTHREAD_MUTEX_PROTOCOL_MASK;
  return 0;
}
int pthread_mutexattr_getrobust(const pthread_mutexattr_t* restrict a, int* restrict robust) {
  *robust = (a->__attr >> PTHREAD_MUTEX_ROBUST_SHIFT) & PTHREAD_MUTEX_ROBUST_MASK;
  return 0;
}

int pthread_mutexattr_gettype(const pthread_mutexattr_t* restrict a, int* restrict type) {
  *type = (a->__attr >> PTHREAD_MUTEX_TYPE_SHIFT) & PTHREAD_MUTEX_TYPE_MASK;
  return 0;
}
