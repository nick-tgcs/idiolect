#!/usr/bin/env bash
set -euo pipefail

perf_dir="target/performance"
startup_threshold_ms=5000
transcription_threshold_ms=120000
max_rss_threshold_kb=1048576

if [[ ! -x /usr/bin/time ]]; then
  echo "required tool missing: /usr/bin/time" >&2
  exit 1
fi

mkdir -p "${perf_dir}"

# Build all artefacts before timing begins so that cache-miss compilation does
# not inflate the latency measurements.
cargo build -p idiolectd -p idiolect-cli
cargo test -p idiolect-integration-tests --all-features --test real_asr_contracts --no-run

start_ns="$(date +%s%N)"
/usr/bin/time -v target/debug/idiolectd --version --json >"${perf_dir}/idiolectd-version.json" 2>"${perf_dir}/idiolectd-time.txt"
end_ns="$(date +%s%N)"
startup_latency_ms=$(( (end_ns - start_ns) / 1000000 ))
max_rss_kb="$(awk -F: '/Maximum resident set size/ { gsub(/^[[:space:]]+/, "", $2); print $2 }' "${perf_dir}/idiolectd-time.txt")"

start_ns="$(date +%s%N)"
cargo test -p idiolect-integration-tests --all-features --test real_asr_contracts whisper_transcribes_fixture_audio
end_ns="$(date +%s%N)"
transcription_latency_ms=$(( (end_ns - start_ns) / 1000000 ))

cat >"${perf_dir}/performance-smoke.txt" <<REPORT
startup_latency_ms=${startup_latency_ms}
startup_threshold_ms=${startup_threshold_ms}
transcription_latency_ms=${transcription_latency_ms}
transcription_threshold_ms=${transcription_threshold_ms}
max_rss_kb=${max_rss_kb}
max_rss_threshold_kb=${max_rss_threshold_kb}
REPORT

if [[ -z "${max_rss_kb}" ]]; then
  echo "failed to record max_rss_kb" >&2
  exit 1
fi
if (( startup_latency_ms <= 0 || startup_latency_ms > startup_threshold_ms )); then
  echo "startup latency outside threshold: ${startup_latency_ms}ms" >&2
  exit 1
fi
if (( transcription_latency_ms <= 0 || transcription_latency_ms > transcription_threshold_ms )); then
  echo "transcription latency outside threshold: ${transcription_latency_ms}ms" >&2
  exit 1
fi
if (( max_rss_kb <= 0 || max_rss_kb > max_rss_threshold_kb )); then
  echo "memory footprint outside threshold: ${max_rss_kb}KB" >&2
  exit 1
fi
