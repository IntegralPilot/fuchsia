#include <locale.h>
#include <string.h>

#include "locale_impl.h"

static const char envvars[][12] = {
    "LC_CTYPE", "LC_NUMERIC", "LC_TIME", "LC_COLLATE", "LC_MONETARY", "LC_MESSAGES",
};

const struct __locale_map* __get_locale(int cat, const char* val) {
  if (!*val) {
    ((val = getenv("LC_ALL")) && *val) || ((val = getenv(envvars[cat])) && *val) ||
        ((val = getenv("LANG")) && *val) || (val = "C.UTF-8");
  }

  if (cat == LC_CTYPE && !strcmp(val, "C.UTF-8")) {
    return (void*)&__c_dot_utf8;
  }

  return 0;
}
