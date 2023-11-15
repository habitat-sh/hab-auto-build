SRC_PATH="${2:?}"
PLAN_CONTEXT="${3:?}"
if [[ -n "${BUILD_PKG_TARGET:-}" ]]; then
  pkg_target="$BUILD_PKG_TARGET"
  unset BUILD_PKG_TARGET
else
  # Otherwise, attempt to detect a suitable value for `$pkg_target` by using
  _pkg_arch="$(uname -m | tr '[:upper:]' '[:lower:]')"
  _pkg_sys="$(uname -s | tr '[:upper:]' '[:lower:]')"
  pkg_target="${_pkg_arch}-${_pkg_sys}"
  unset _pkg_arch _pkg_sys
fi

source "${1:?}"

if [[ "$(type -t pkg_version)" == "function" || -z "${pkg_version}" ]]; then
  pkg_version="**DYNAMIC**"
fi

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

# Output data as json
echo "{ \
\"origin\": \"${pkg_origin}\", \
\"name\": \"${pkg_name}\", \
\"version\": \"${pkg_version}\","

if [[ ! -z "${pkg_shasum}" ]]; then
  echo "\"source\": { \
  \"url\": \"${pkg_source}\",\
  \"shasum\": \"${pkg_shasum}\" \
  },"
fi

if [[ ! -z "${pkg_license}" ]]; then
  echo "\"licenses\": $(json_array "${pkg_license[@]}"),"
else
  echo "\"licenses\": [],"
fi

if [[ ! -z "${pkg_scaffolding}" ]]; then
  echo "\"scaffolding_dep\": \"${pkg_scaffolding}\","
else
  echo "\"scaffolding_dep\": null,"
fi

echo "\"deps\": $(json_array "${pkg_deps[@]}"), \
\"build_deps\": $(json_array "${pkg_build_deps[@]}")\
}"
