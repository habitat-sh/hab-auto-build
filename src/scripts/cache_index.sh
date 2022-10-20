#!/bin/bash
shopt -s nullglob

for file in /hab/cache/artifacts/"$1-$2"-*; do
    ident=$(tail -n +6 < "$file" | tar -Jtf - | head -n 1)
    ident="${ident#"hab/pkgs/"}"
    ident="${ident%"/"}"
    name="${ident#"$1"/}"
    name="${name%/*/*}"
    if [[ $name == "$2" ]]; then
        file="${file#"/hab/cache/artifacts/"}"
        echo "$ident=$file"
    fi
done
