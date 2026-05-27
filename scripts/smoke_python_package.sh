#!/usr/bin/env sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

python_bin=${PYTHON:-python3}
tmp_root=${TMPDIR:-/tmp}
tmp_dir=$(mktemp -d "$tmp_root/callbook-python-smoke.XXXXXX")
cleanup() {
    rm -rf "$tmp_dir"
}
trap cleanup EXIT INT TERM

"$python_bin" -m venv "$tmp_dir/venv"

if [ -x "$tmp_dir/venv/bin/python" ]; then
    venv_python="$tmp_dir/venv/bin/python"
else
    venv_python="$tmp_dir/venv/Scripts/python.exe"
fi

"$venv_python" -m pip install ./python
"$venv_python" -m unittest discover -s python/tests
"$venv_python" -c "import callbook_rs; from callbook_rs import CallBook; print(callbook_rs.__version__); print(CallBook.__name__)"
