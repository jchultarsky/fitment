#!/bin/sh
# Representative sockets at three catalog sizes, SQL level 1, 1 and 16 threads; mem level 1 at 16.
P=./proto/target/release/probe
for db in c3 c4 c5; do
  for k in s01_bolted__BRACKET s01_bolted__NUT s02_cbore__BLOCK s05_stack__WASHER s07b_one_missing__BRACKET s19_boss__HOUSING s03_dowels__PLATE; do
    for th in 1 16; do
      printf "%s %-28s t%-2s sql " $db $k $th; $P query db/$db.db sockets/$k.json $th out/tmp.json | tail -1
    done
    printf "%s %-28s t16 mem " $db $k; $P query db/$db.db sockets/$k.json 16 out/tmp.json mem | tail -1
  done
done
