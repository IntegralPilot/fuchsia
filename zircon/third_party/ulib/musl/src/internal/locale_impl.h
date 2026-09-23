#ifndef ZIRCON_THIRD_PARTY_ULIB_MUSL_SRC_INTERNAL_LOCALE_IMPL_H_
#define ZIRCON_THIRD_PARTY_ULIB_MUSL_SRC_INTERNAL_LOCALE_IMPL_H_

#include <locale.h>
#include <stdlib.h>

#include "libc.h"
#include "threads_impl.h"

#define LOCALE_NAME_MAX 15

struct __locale_map {
  const void* map;
  size_t map_size;
  char name[LOCALE_NAME_MAX + 1];
  const struct __locale_map* next;
};

extern const struct __locale_map __c_dot_utf8;
extern const struct __locale_struct __c_locale;
extern const struct __locale_struct __c_dot_utf8_locale;

const struct __locale_map* __get_locale(int, const char*) ATTR_LIBC_VISIBILITY;
int __loc_is_allocated(locale_t) ATTR_LIBC_VISIBILITY;

#define LCTRANS(msg, lc, loc) (msg)
#define LCTRANS_CUR(msg) (msg)

#define C_LOCALE ((locale_t)&__c_locale)
#define UTF8_LOCALE ((locale_t)&__c_dot_utf8_locale)

#define CURRENT_LOCALE (__thrd_current()->locale)

#define CURRENT_UTF8 (!!__thrd_current()->locale->cat[LC_CTYPE])

#undef MB_CUR_MAX
#define MB_CUR_MAX (CURRENT_UTF8 ? 4 : 1)

#endif  // ZIRCON_THIRD_PARTY_ULIB_MUSL_SRC_INTERNAL_LOCALE_IMPL_H_
