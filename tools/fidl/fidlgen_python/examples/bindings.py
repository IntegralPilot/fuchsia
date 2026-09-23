# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.


# [START bits]
import fidl_test_bits

a = fidl_test_bits.MyBits.MY_FIRST_BIT
b = fidl_test_bits.MyBits(0b10)
c = fidl_test_bits.MyBits(0b10000101)

print(f"{a!r}")
print(f"{b!r}")
print(f"{c!r}")
# [END bits]

# [START struct]
import fidl_test_struct

d = fidl_test_struct.Simple(f1=5, f2=True)

print(f"{d!r}")
# [END struct]

# [START table]
import fidl_test_table

e = fidl_test_table.EmptyTable()
f = fidl_test_table.SimpleTable(x=6)
g = fidl_test_table.SimpleTable(y=7)
h = fidl_test_table.SimpleTable()

print(f"{e!r}")
print(f"{f!r}")
print(f"{g!r}")
print(f"{h!r}")
# [END table]

# [START union]
import fidl_test_union

i = fidl_test_union.PizzaOrPasta(
    pizza=fidl_test_union.Pizza(toppings=["pepperoni", "jalapeños"])
)
j = fidl_test_union.PizzaOrPasta(pasta=fidl_test_union.Pasta(sauce="pesto"))

print(f"{i!r}")
print(f"{j!r}")
# [END union]


def main() -> None:
    ...
