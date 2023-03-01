source "${1:?}"

if [[ "$(type -t pkg_version)" == "function" ]]; then
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

if [[ ! -z "${pkg_source}" ]]; then
  echo "\"source\": { \
  \"url\": \"${pkg_source}\",\
  \"shasum\": \"${pkg_shasum}\" \
  },"
fi

if [[ ! -z "${pkg_license}" ]]; then
  echo "\"licenses\": $(json_array ${pkg_license[@]}),"
else
  echo "\"licenses\": [],"
fi

echo "\"deps\": $(json_array ${pkg_deps[@]}), \
\"build_deps\": $(json_array ${pkg_build_deps[@]})\
}"
