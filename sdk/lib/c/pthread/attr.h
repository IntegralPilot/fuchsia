// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_PTHREAD_ATTR_H_
#define LIB_C_PTHREAD_ATTR_H_

#include <pthread.h>

#include "../threads/thread.h"

namespace LIBC_NAMESPACE_DECL {

constexpr pthread_attr_t ToPthreadAttr(ThreadAttributes attr) {
  return {
      // TODO(https://fxbug.dev/473624022): POSIX requires that unrounded
      // values be returned by get* calls exactly as set by set* calls; it
      // permits rounding only at time of use
      ._a_stacksize = attr.stack.get(),
      ._a_guardsize = attr.guard.get(),
      ._a_detach = attr.detached,
  };
}

inline ThreadAttributes FromPthreadAttr(const pthread_attr_t* attr) {
  ThreadAttributes attrs;
  if (attr) {
    // These have been checked for overflow in pthread_attr_set so this is unconditionally safe
    // here.
    attrs.stack = *PageRoundedSize::From(attr->_a_stacksize);
    attrs.guard = *PageRoundedSize::From(attr->_a_guardsize);
    attrs.detached = attr->_a_detach;
    if (attr->__name) {
      attrs.name = ZxName{attr->__name};
    }
  }
  return attrs;
}

}  // namespace LIBC_NAMESPACE_DECL

#endif  // LIB_C_PTHREAD_ATTR_H_
