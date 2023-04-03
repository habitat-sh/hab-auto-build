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

echo "{"
echo "\"licenses\": $(json_array "${pkg_license[@]}")"
echo "}"