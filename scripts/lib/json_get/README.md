# JsonGet - match and extract data ergonomically from JSON lists and dicts

json_get has one class, JsonGet, with one method, match().

## Create the JSON object

JsonGet('{"string": "formatted in JSON"}')

JsonGet(value=["Any", "JSON-like", "data"])

## Match and extract data

Match by passing a data structure into ``match() - there's no separate DSL. If the data structure
matches, a JsonMatch object will be returned, containing the matched data; if the match fails,
None will be returned.

Wildcards for dictionary keys:
 - `Any` matches any data. If used as a value in a dictionary, the key must exist.
 - `Maybe` matches any data, and if a key is missing, that key will be added with value `None`.
 - `None` as a value in a dictionary requires that the key _not_ be present.

Any is also a wildcard for list patterns, and for matching any data object.

`Any` and `Maybe` return shallow copies of the data they match, so the original structure can be
modified.

`    def match(self, pattern, callback=None, no_match=None)`

In addition, a 1-argument function passed as `callback` will be called with a successful match,
and a 0-argument function passed as `no_match` will be called if the match fails.

```
from json_get import JsonGet, Any, Maybe  # Any is typing.Any

j = JsonGet('{"a": 1, "b": [7, 8, {"nested": 42}]})

print(j.match({"a": Any}).a)   # prints 1
print(j.match({"a": Any}).b)   # Error - the returned object doesn't have a field `b`
print(j.match({"b": [{"nested": Any}])).b.nested  # prints 42
print(j.match({"b": [7])).b  # prints [7]
print(j.match({"b": [Any])).b  # prints [7, 8, {"nested": 42}]
```

### Dictionary patterns

`Maybe` and `None` are only meaningful as dictionary values. Keys must match exactly, and must be
valid Python object-member names.

### List patterns

A list in a pattern must contain exactly one item. The corresponding data must be a list. Each
element of that list that matches the pattern will be added to the returned list. If nothing
matches, the returned value is [] rather than None. To match an entire list, use [Any].
