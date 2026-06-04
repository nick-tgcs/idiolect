#!/usr/bin/env bash
# Download Whisper ggml model files from https://huggingface.co/ggerganov/whisper.cpp
#
# Model selection guide (see detailed comments in the case statement below):
#
#   Size tier trade-offs (OpenAI Whisper benchmarks on English, A100 relative speed):
#     tiny   ~75 MiB  | ~10x speed | ~1 GB RAM | Lowest accuracy, fastest
#     base   ~142 MiB |  ~7x speed | ~1 GB RAM | Slight accuracy gain over tiny
#     small  ~466 MiB |  ~4x speed | ~2 GB RAM | Good balance for many tasks
#     medium ~1.5 GiB |  ~2x speed | ~5 GB RAM | High accuracy, slower
#     large  ~2.9 GiB |   1x speed | ~10 GB RAM| Best accuracy, slowest
#
#   .en suffix  = English-only; generally better WER on English than the
#                 multilingual counterpart at the same size tier.
#   -q5_0 / -q5_1 = 5-bit quantised; much smaller, slight accuracy loss.
#                    q5_1 preserves more precision than q5_0.
#   -q8_0        = 8-bit quantised; good compression with minimal accuracy loss.
#   -tdrz        = tinydiarize; adds speaker-turn detection (small.en-tdrz only).
#   large-v3-turbo = Optimised large-v3; ~8x speed of large, near-large accuracy.
#                    NOTE: turbo does NOT support translation tasks; use medium/large
#                    for non-English→English translation.
#
#   Recommended defaults for this project:
#     CI / fast tests      → tiny.en   (smallest, fastest, English-only)
#     Integration tests    → base.en   (slightly better accuracy, still fast)
#     Production accuracy  → large-v3-turbo or large-v3 (best WER, multilingual)
#
set -euo pipefail

model="${1:-tiny}"

case "$model" in
  # ── Tiny (39 M params) ──────────────────────────────────────────────────
  # Smallest model. Best for CI pipelines and quick smoke tests where
  # accuracy is secondary to speed. The .en variant noticeably outperforms
  # the multilingual tiny on English audio.
  tiny)
    file_name="ggml-tiny.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin"
    sha256="bd577a113a864445d4c299885e0cb97d4ba92b5f"
    ;;
  tiny.en)
    file_name="ggml-tiny.en.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin"
    sha256="c78c86eb1a8faa21b369bcd33207cc90d64ae9df"
    ;;
  # 5-bit quantised tiny — ~31 MiB. Good when bandwidth/disk is extremely
  # constrained and some accuracy loss is acceptable.
  tiny-q5_1)
    file_name="ggml-tiny-q5_1.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny-q5_1.bin"
    sha256="2827a03e495b1ed3048ef28a6a4620537db4ee51"
    ;;
  tiny.en-q5_1)
    file_name="ggml-tiny.en-q5_1.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en-q5_1.bin"
    sha256="3fb92ec865cbbc769f08137f22470d6b66e071b6"
    ;;
  # 8-bit quantised tiny — ~42 MiB. Better accuracy than q5_1, still much
  # smaller than full-precision tiny.
  tiny-q8_0)
    file_name="ggml-tiny-q8_0.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny-q8_0.bin"
    sha256="19e8118f6652a650569f5a949d962154e01571d9"
    ;;
  tiny.en-q8_0)
    file_name="ggml-tiny.en-q8_0.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en-q8_0.bin"
    sha256="802d6668e7d411123e672abe4cb6c18f12306abb"
    ;;

  # ── Base (74 M params) ──────────────────────────────────────────────────
  # Meaningful accuracy improvement over tiny while still being fast.
  # Good default for integration tests and lightweight local development.
  # The .en variant is recommended when only English transcription is needed.
  base)
    file_name="ggml-base.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin"
    sha256="465707469ff3a37a2b9b8d8f89f2f99de7299dac"
    ;;
  base.en)
    file_name="ggml-base.en.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin"
    sha256="137c40403d78fd54d454da0f9bd998f78703390c"
    ;;
  base-q5_1)
    file_name="ggml-base-q5_1.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base-q5_1.bin"
    sha256="a3733eda680ef76256db5fc5dd9de8629e62c5e7"
    ;;
  base.en-q5_1)
    file_name="ggml-base.en-q5_1.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en-q5_1.bin"
    sha256="d26d7ce5a1b6e57bea5d0431b9c20ae49423c94a"
    ;;
  base-q8_0)
    file_name="ggml-base-q8_0.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base-q8_0.bin"
    sha256="7bb89bb49ed6955013b166f1b6a6c04584a20fbe"
    ;;
  base.en-q8_0)
    file_name="ggml-base.en-q8_0.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en-q8_0.bin"
    sha256="bb1574182e9b924452bf0cd1510ac034d323e948"
    ;;

  # ── Small (244 M params) ────────────────────────────────────────────────
  # Good balance of accuracy and resource usage. The .en variant is
  # competitive with medium.en on English WER while being ~3× faster.
  # small.en-tdrz adds speaker-turn detection via tinydiarize — useful
  # for diarization tasks but not needed for plain transcription.
  small)
    file_name="ggml-small.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin"
    sha256="55356645c2b361a969dfd0ef2c5a50d530afd8d5"
    ;;
  small.en)
    file_name="ggml-small.en.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en.bin"
    sha256="db8a495a91d927739e50b3fc1cc4c6b8f6c2d022"
    ;;
  # tinydiarize variant — marks speaker turns with [SPEAKER_TURN] tokens.
  # Only available for small.en. Use when you need basic diarization.
  small.en-tdrz)
    file_name="ggml-small.en-tdrz.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en-tdrz.bin"
    sha256="b6c6e7e89af1a35c08e6de56b66ca6a02a2fdfa1"
    ;;
  small-q5_1)
    file_name="ggml-small-q5_1.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q5_1.bin"
    sha256="6fe57ddcfdd1c6b07cdcc73aaf620810ce5fc771"
    ;;
  small.en-q5_1)
    file_name="ggml-small.en-q5_1.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en-q5_1.bin"
    sha256="20f54878d608f94e4a8ee3ae56016571d47cba34"
    ;;
  small-q8_0)
    file_name="ggml-small-q8_0.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q8_0.bin"
    sha256="bcad8a2083f4e53d648d586b7dbc0cd673d8afad"
    ;;
  small.en-q8_0)
    file_name="ggml-small.en-q8_0.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en-q8_0.bin"
    sha256="9d75ff4ccfa0a8217870d7405cf8cef0a5579852"
    ;;

  # ── Medium (769 M params) ───────────────────────────────────────────────
  # High accuracy, especially for non-English languages. The .en variant
  # is only marginally better than small.en on English, so prefer small.en
  # unless you need the extra accuracy on multilingual audio.
  # Requires ~5 GB RAM; may be slow on CPU-only systems.
  medium)
    file_name="ggml-medium.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin"
    sha256="fd9727b6e1217c2f614f9b698455c4ffd82463b4"
    ;;
  medium.en)
    file_name="ggml-medium.en.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.en.bin"
    sha256="8c30f0e44ce9560643ebd10bbe50cd20eafd3723"
    ;;
  medium-q5_0)
    file_name="ggml-medium-q5_0.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium-q5_0.bin"
    sha256="7718d4c1ec62ca96998f058114db98236937490e"
    ;;
  medium.en-q5_0)
    file_name="ggml-medium.en-q5_0.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.en-q5_0.bin"
    sha256="bb3b5281bddd61605d6fc76bc5b92d8f20284c3b"
    ;;
  medium-q8_0)
    file_name="ggml-medium-q8_0.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium-q8_0.bin"
    sha256="e66645948aff4bebbec71b3485c576f3d63af5d6"
    ;;
  medium.en-q8_0)
    file_name="ggml-medium.en-q8_0.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.en-q8_0.bin"
    sha256="b1cf48c12c807e14881f634fb7b6c6ca867f6b38"
    ;;

  # ── Large-v1 (1550 M params) ────────────────────────────────────────────
  # Original large model. Superseded by large-v2 and large-v3 — prefer those
  # unless you specifically need v1 for reproducibility.
  large-v1)
    file_name="ggml-large-v1.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v1.bin"
    sha256="b1caaf735c4cc1429223d5a74f0f4d0b9b59a299"
    ;;

  # ── Large-v2 (1550 M params) ────────────────────────────────────────────
  # Improved large model (Dec 2022). Better multilingual accuracy than v1.
  # For most production use-cases, prefer large-v3 or large-v3-turbo instead.
  large-v2)
    file_name="ggml-large-v2.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v2.bin"
    sha256="0f4c8e34f21cf1a914c59d8b3ce882345ad349d6"
    ;;
  large-v2-q5_0)
    file_name="ggml-large-v2-q5_0.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v2-q5_0.bin"
    sha256="00e39f2196344e901b3a2bd5814807a769bd1630"
    ;;
  large-v2-q8_0)
    file_name="ggml-large-v2-q8_0.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v2-q8_0.bin"
    sha256="da97d6ca8f8ffbeeb5fd147f79010eeea194ba38"
    ;;

  # ── Large-v3 (1550 M params) ────────────────────────────────────────────
  # Best-in-class accuracy (Nov 2023). Best WER across all languages.
  # Requires ~10 GB RAM; slow on CPU-only. Use large-v3-turbo if you need
  # faster inference with near-v3 accuracy.
  large-v3)
    file_name="ggml-large-v3.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin"
    sha256="ad82bf6a9043ceed055076d0fd39f5f186ff8062"
    ;;
  large-v3-q5_0)
    file_name="ggml-large-v3-q5_0.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-q5_0.bin"
    sha256="e6e2ed78495d403bef4b7cff42ef4aaadcfea8de"
    ;;

  # ── Large-v3-turbo (809 M params) ────────────────────────────────────────
  # Optimised distillation of large-v3 (Sep 2024). ~8× faster than large
  # with only minimal WER degradation. Best choice for production when
  # you need high accuracy at reasonable speed.
  # ⚠ Does NOT support translation tasks (non-English → English);
  #   use medium or large-v3 for that.
  large-v3-turbo)
    file_name="ggml-large-v3-turbo.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin"
    sha256="4af2b29d7ec73d781377bfd1758ca957a807e941"
    ;;
  large-v3-turbo-q5_0)
    file_name="ggml-large-v3-turbo-q5_0.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin"
    sha256="e050f7970618a659205450ad97eb95a18d69c9ee"
    ;;
  large-v3-turbo-q8_0)
    file_name="ggml-large-v3-turbo-q8_0.bin"
    url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q8_0.bin"
    sha256="01bf15bedffe9f39d65c1b6ff9b687ea91f59e0e"
    ;;

  *)
    cat >&2 <<'HELP'
usage: $0 <model>

Available models (source: https://huggingface.co/ggerganov/whisper.cpp):

  Tiny (~75 MiB) — fastest, lowest accuracy; good for CI smoke tests
    tiny             tiny.en           tiny-q5_1
    tiny.en-q5_1     tiny-q8_0         tiny.en-q8_0

  Base (~142 MiB) — modest accuracy gain over tiny; good for integration tests
    base             base.en           base-q5_1
    base.en-q5_1     base-q8_0         base.en-q8_0

  Small (~466 MiB) — good accuracy/speed trade-off; .en-tdrz adds diarization
    small            small.en          small.en-tdrz
    small-q5_1       small.en-q5_1     small-q8_0
    small.en-q8_0

  Medium (~1.5 GiB) — high accuracy; needed for good multilingual WER
    medium           medium.en         medium-q5_0
    medium.en-q5_0   medium-q8_0       medium.en-q8_0

  Large-v1 (~2.9 GiB) — original large; superseded by v2/v3
    large-v1

  Large-v2 (~2.9 GiB) — improved large; prefer v3 for new work
    large-v2         large-v2-q5_0     large-v2-q8_0

  Large-v3 (~2.9 GiB) — best accuracy; slow on CPU
    large-v3         large-v3-q5_0

  Large-v3-turbo (~1.5 GiB) — near-v3 accuracy at ~8× speed; no translation
    large-v3-turbo   large-v3-turbo-q5_0  large-v3-turbo-q8_0

Suffix guide:
  .en       English-only (better WER on English than multilingual counterpart)
  -q5_0     5-bit quantised (smallest, most accuracy loss)
  -q5_1     5-bit quantised (slightly better precision than q5_0)
  -q8_0     8-bit quantised (good compression, minimal accuracy loss)
  -tdrz     tinydiarize speaker-turn detection (small.en only)
HELP
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
