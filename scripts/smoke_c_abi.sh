#!/usr/bin/env sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

tmp_root=${TMPDIR:-/tmp}
tmp_dir=$(mktemp -d "$tmp_root/callbook-c-abi-smoke.XXXXXX")
cleanup() {
    rm -rf "$tmp_dir"
}
trap cleanup EXIT INT TERM

cargo build --release -p callbook-rs --features cli

target_dir=$(
    python3 -c 'import json, subprocess; print(json.loads(subprocess.check_output(["cargo", "metadata", "--no-deps", "--format-version", "1"]))["target_directory"])'
)
release_dir="$target_dir/release"

db_dir="$tmp_dir/db"
mkdir -p "$db_dir/ham0"
printf 'headerrecord' > "$db_dir/ham0/hamcall.dat"
printf '!!! 0 \r\nK0AB 6 \r\nZZZZZZZZ 11 \r\n' > "$db_dir/ham0/hamcall.idx"

cc -std=c99 -Wall -Wextra \
    -I crates/callbook/include \
    crates/callbook/tests/c_abi.c \
    -L "$release_dir" \
    -lcallbook \
    -o "$tmp_dir/c_abi_smoke"

case "$(uname -s)" in
    Darwin)
        CALLBOOK_DB="$db_dir" DYLD_LIBRARY_PATH="$release_dir${DYLD_LIBRARY_PATH:+:$DYLD_LIBRARY_PATH}" "$tmp_dir/c_abi_smoke" K0AB
        ;;
    Linux)
        CALLBOOK_DB="$db_dir" LD_LIBRARY_PATH="$release_dir${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" "$tmp_dir/c_abi_smoke" K0AB
        ;;
    *)
        CALLBOOK_DB="$db_dir" "$tmp_dir/c_abi_smoke" K0AB
        ;;
esac
