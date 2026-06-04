#!/usr/bin/env bash
set -euo pipefail

model="${1:-tiny}"

case "$model" in
  tiny)
    file_name="ggml-tiny.en.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin"
    sha256="921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f"
    ;;
  base)
    file_name="ggml-base.en.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/refs%2Fpr%2F8/ggml-base.en.bin"
    sha256="a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002"
    ;;
  *)
    echo "usage: $0 [tiny|base]" >&2
    exit 1
    ;;
esac

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
dest_dir="$repo_root/tests/fixtures/whisper"
tmp_file="$(mktemp)"

trap 'rm -f "$tmp_file"' EXIT

mkdir -p "$dest_dir"
curl -fL --retry 3 --retry-delay 2 --output "$tmp_file" "$url"
printf '%s  %s\n' "$sha256" "$tmp_file" | sha256sum -c -
mv "$tmp_file" "$dest_dir/$file_name"
trap - EXIT
