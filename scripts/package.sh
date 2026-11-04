#!/usr/bin/env bash
# Build the plugin for one target and produce the plugin-store release asset:
#   dist/<id>_<version>_<goos>_<goarch>.zip containing <id>.<ext> at the zip root.
# usage: scripts/package.sh <rust-target> <goos> <goarch>
set -euo pipefail
target=$1 goos=$2 goarch=$3
id=cpa-pools-and-aliases
version=$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)

cargo build --release --locked --target "$target"

case "$goos" in
  darwin)  src=libcpa_pools_and_aliases.dylib; ext=dylib ;;
  windows) src=cpa_pools_and_aliases.dll;      ext=dll ;;
  *)       src=libcpa_pools_and_aliases.so;    ext=so ;;
esac

mkdir -p dist
cp "target/$target/release/$src" "dist/$id.$ext"
zip="dist/${id}_${version}_${goos}_${goarch}.zip"
rm -f "$zip"
python3 - "$zip" "dist/$id.$ext" "$id.$ext" <<'PY'
import sys, zipfile
with zipfile.ZipFile(sys.argv[1], "w", zipfile.ZIP_DEFLATED) as z:
    z.write(sys.argv[2], sys.argv[3])
PY
echo "packaged $zip"
