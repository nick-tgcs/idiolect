#!/usr/bin/env bash
set -euo pipefail

coverage_map="docs/quality/v1-coverage-map.md"
coverage_gate="ci/scripts/test-all.sh"
cpp_source_dir="fcitx5/idiolect-fcitx5"
cpp_cmake_file="${cpp_source_dir}/CMakeLists.txt"
cpp_build_dir="${cpp_source_dir}/build"

required_processes=(
  "audio.capture"
  "audio.fixture"
  "codec.opus"
  "vad.segment"
  "asr.whisper"
  "daemon.startup"
  "ipc.handshake"
  "ipc.lifecycle"
  "fcitx5.preedit"
  "fcitx5.commit"
  "fcitx5.cancel"
  "storage.event_log"
  "storage.materialized_tables"
  "candidate.capture"
  "learning.classifier"
  "learning.manifest"
  "learning.promotion"
  "learning.rollback"
  "privacy.export"
  "privacy.delete"
  "privacy.deleted_data_excluded"
  "package.payload"
  "package.smoke"
)

if [[ ! -f "$coverage_map" ]]; then
  echo "coverage map missing: $coverage_map" >&2
  exit 1
fi

if [[ ! -f "$coverage_gate" ]]; then
  echo "coverage gate missing: $coverage_gate" >&2
  exit 1
fi

if ! rg -q "prototype baseline evidence" "$coverage_map"; then
  echo "coverage map must state it is prototype baseline evidence, not v1 completion evidence" >&2
  exit 1
fi

if rg -q "UNASSIGNED" "$coverage_map"; then
  echo "coverage map contains UNASSIGNED rows" >&2
  exit 1
fi

scan_for_suppressed_tests_and_lints() {
  local cpp_scan_paths=()

  if rg -n "#!?\[[^]]*\b(ignore|allow|expect)\b" crates --glob "*.rs"; then
    echo "suppressed or ignored Rust tests/lints are not allowed" >&2
    exit 1
  fi

  [[ -d "${cpp_source_dir}/src" ]] && cpp_scan_paths+=("${cpp_source_dir}/src")
  [[ -d "${cpp_source_dir}/tests" ]] && cpp_scan_paths+=("${cpp_source_dir}/tests")

  if (( ${#cpp_scan_paths[@]} > 0 )) && rg -n "\b(NOLINT|GTEST_SKIP|DISABLED_[[:alnum:]_]+)\b" "${cpp_scan_paths[@]}"; then
    echo "suppressed or skipped C++ tests/lints are not allowed" >&2
    exit 1
  fi

  if [[ -f "$cpp_cmake_file" ]] && rg -n "\b(DISABLED|SKIP_RETURN_CODE|WILL_FAIL)\b" "$cpp_cmake_file"; then
    echo "suppressed or skipped CTest behavior is not allowed" >&2
    exit 1
  fi
}

trim() {
  local value="$1"
  value="${value#${value%%[![:space:]]*}}"
  value="${value%${value##*[![:space:]]}}"
  printf '%s' "$value"
}

declare -A invoked_gate_scripts=()

parse_coverage_gate() {
  local line
  local command
  local first_arg

  while IFS= read -r line; do
    line="$(trim "$line")"

    [[ -z "$line" ]] && continue
    [[ "$line" == "#!/usr/bin/env bash" ]] && continue
    [[ "$line" == "set -euo pipefail" ]] && continue
    [[ "$line" == \#* ]] && continue

    read -r command first_arg _ <<< "$line"
    if [[ "$command" == "bash" && "$first_arg" == ci/scripts/*.sh ]]; then
      invoked_gate_scripts["$first_arg"]=1
      continue
    fi

    echo "coverage gate contains unsupported command line: $line" >&2
    exit 1
  done < "$coverage_gate"
}

rust_test_exists() {
  local test_id="$1"
  rg -U -q "#\\[(test|tokio::test)(\\([^]]*\\))?\\][[:space:]]*(#\\[[^]]+\\][[:space:]]*)*fn[[:space:]]+${test_id}[[:space:]]*\\(" crates --glob '*.rs'
}

refresh_cpp_ctest_index() {
  [[ -f "$cpp_cmake_file" ]] || return 1
  command -v cmake >/dev/null 2>&1 || return 1

  cmake \
    -S "$cpp_source_dir" \
    -B "$cpp_build_dir" \
    -DCMAKE_BUILD_TYPE=RelWithDebInfo \
    -DCMAKE_CXX_FLAGS="-Wall -Wextra -Wpedantic -Werror" \
    >/dev/null
}

cpp_test_exists() {
  local test_id="$1"

  if rg -U -q "add_test\\([[:space:]]*NAME[[:space:]]+${test_id}\\b" "$cpp_cmake_file"; then
    return 0
  fi

  if refresh_cpp_ctest_index && command -v ctest >/dev/null 2>&1; then
    if ctest --test-dir "$cpp_build_dir" -N 2>/dev/null | rg -q "^[[:space:]]*Test #[[:digit:]]+: ${test_id}\\b"; then
      return 0
    fi
  fi

  return 1
}

scan_for_suppressed_tests_and_lints
parse_coverage_gate

declare -A process_counts=()
declare -A process_tests=()

while IFS= read -r line; do
  [[ ! "$line" == "|"* ]] && continue

  if [[ "$line" =~ ^\|[[:space:]]*process[[:space:]]*\|[[:space:]]*automated_test[[:space:]]*\| ]]; then
    continue
  fi
  if [[ "$line" =~ ^\|[[:space:]]*-+[[:space:]]*\|[[:space:]]*-+[[:space:]]*\| ]]; then
    continue
  fi

  IFS='|' read -r _ process automated_test _ <<< "$line"
  process="$(trim "$process")"
  automated_test="$(trim "$automated_test")"

  [[ -z "$process" ]] && continue

  process_counts["$process"]=$(( ${process_counts["$process"]:-0} + 1 ))
  process_tests["$process"]="$automated_test"

  if [[ -z "$automated_test" ]]; then
    echo "coverage map has blank automated_test for process: $process" >&2
    exit 1
  fi
done < "$coverage_map"

for process in "${required_processes[@]}"; do
  count="${process_counts[$process]:-0}"
  if [[ "$count" -eq 0 ]]; then
    echo "coverage map missing required process: $process" >&2
    exit 1
  fi
  if [[ "$count" -ne 1 ]]; then
    echo "coverage map must include required process exactly once: $process (found $count)" >&2
    exit 1
  fi

  automated_test="${process_tests[$process]}"

  if [[ "$automated_test" == "UNASSIGNED" ]]; then
    echo "coverage map has UNASSIGNED automated_test for process: $process" >&2
    exit 1
  fi

  if [[ "$automated_test" == ci/scripts/*.sh ]]; then
    if [[ ! -f "$automated_test" ]]; then
      echo "coverage map references missing script: $automated_test" >&2
      exit 1
    fi
    if [[ ! -x "$automated_test" ]]; then
      echo "coverage map references non-executable script: $automated_test" >&2
      exit 1
    fi
    if [[ -z "${invoked_gate_scripts[$automated_test]:-}" ]]; then
      echo "coverage gate missing required script invocation: $automated_test" >&2
      exit 1
    fi
    continue
  fi

  if rust_test_exists "$automated_test"; then
    continue
  fi

  if cpp_test_exists "$automated_test"; then
    continue
  fi

  echo "coverage map references unresolved automated test: $automated_test (process: $process)" >&2
  exit 1
done
