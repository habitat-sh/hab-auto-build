#!/bin/bash
shopt -s nullglob

for file in /hab/cache/artifacts/"$1"-*; do
    ident=$(tail -n +6 < "$file" | tar -Jtf - | head -n 1)
    ident="${ident#"hab/pkgs/"}"
    ident="${ident%"/"}"
    file="${file#"/hab/cache/artifacts/"}"
    echo "$ident=$file"
done
