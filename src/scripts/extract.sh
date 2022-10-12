#!/bin/bash

if [[ -n "${BUILD_PKG_TARGET:-}" ]]; then
    # If a build environment variable is set with the desired package target,
    # then update the value of `$pkg_target`. This case is used in
    # bootstrapping the Habitat packaging system.
    pkg_target="$BUILD_PKG_TARGET"
    unset BUILD_PKG_TARGET
else
    # Otherwise, attempt to detect a suitable value for `$pkg_target` by using
    # the `uname` program. This is prior behavior and is backwards compatible
    # and behavior-preserving.
    _pkg_arch="$(uname -m | tr '[:upper:]' '[:lower:]')"
    _pkg_sys="$(uname -s | tr '[:upper:]' '[:lower:]')"
    pkg_target="${_pkg_arch}-${_pkg_sys}"
    unset _pkg_arch _pkg_sys
fi

source "${1:?}"

json_array() {
  echo -n '['
  while [ $# -gt 0 ]; do
    x=${1//\\/\\\\}
    echo -n \""${x//\"/\\\"}"\"
    [ $# -gt 1 ] && echo -n ', '
    shift
  done
  echo ']'
}

echo "{ \
\"path\": \"$(readlink -f "$1")\", \
\"source\": \"$(readlink -f "$2")\", \
\"repo\": \"$(readlink -f "$3")\", \
\"ident\": {\
\"origin\": \"${pkg_origin}\", \
\"name\": \"${pkg_name}\", \
\"version\": \"${pkg_version}\", \
\"target\": \"${pkg_target}\" \
}, \
\"deps\": $(json_array ${pkg_deps[@]}), \
\"build_deps\": $(json_array ${pkg_build_deps[@]})\
}"