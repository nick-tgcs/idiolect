#!/usr/bin/env bash
set -euo pipefail

coverage_map="docs/quality/v1-coverage-map.md"
acceptance_evidence="docs/quality/v1-acceptance-evidence.md"
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
  "package.lifecycle"
  "e2e.failure_recovery"
  "model.regression"
  "performance.smoke"
  "acceptance.evidence"
)

required_acceptance_ids=(
  "functional.app_matrix"
  "functional.preedit_before_commit"
  "functional.correction_edit_events"
  "functional.commit_exact_text"
  "functional.cancel_no_commit"
  "functional.retry_no_duplicate"
  "functional.daemon_restart_store_safe"
  "functional.safe_failure_errors"
  "learning.unchanged_weak_candidate"
  "learning.preedit_correction_high_trust"
  "learning.semantic_rewrite_rejected"
  "learning.holdout_excluded"
  "learning.correction_memory_improves"
  "learning.promotion_gates"
  "learning.rollback_restores"
  "architecture.no_backend_type_leaks"
  "architecture.mock_adapter_every_port"
  "architecture.adapter_contracts"
  "architecture.replace_asr_no_core_change"
  "architecture.replace_storage_no_core_change"
  "architecture.replace_fcitx5_no_core_change"
  "privacy.normal_typing_not_stored"
  "privacy.clipboard_not_read"
  "privacy.no_cloud_default"
  "privacy.raw_transcript_logs_off"
  "privacy.delete_removes_private_data"
  "privacy.strict_excludes_deleted"
  "packaging.installs_components"
  "packaging.uninstall_preserves_user_data"
  "packaging.upgrade_migrates_schema"
  "packaging.clean_vm_install_uninstall"
  "all_done.complete_loop"
  "all_done.replaceable_dependencies"
  "all_done.privacy_controls"
  "all_done.adapter_promotion_rollback"
  "all_done.release_blocking_tests"
  "all_done.target_linux_packaging"
  "all_done.delete_export_data"
  "all_done.failure_modes_safe"
  "gate.input_method_complete"
  "gate.interface_architecture_complete"
  "gate.adapter_replaceability_complete"
  "gate.dictation_loop_complete"
  "gate.storage_complete"
  "gate.learning_loop_complete"
  "gate.privacy_complete"
  "gate.packaging_complete"
  "gate.test_suite_complete"
  "gate.documentation_complete"
)

if [[ ! -f "$coverage_map" ]]; then
  echo "coverage map missing: $coverage_map" >&2
  exit 1
fi

if [[ ! -f "$acceptance_evidence" ]]; then
  echo "acceptance evidence missing: $acceptance_evidence" >&2
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

if rg -q "UNASSIGNED" "$coverage_map" "$acceptance_evidence"; then
  echo "coverage or acceptance evidence contains UNASSIGNED rows" >&2
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

script_from_command() {
  local command_text="$1"
  local shell_cmd
  local script_path
  read -r shell_cmd script_path _ <<< "$command_text"
  if [[ "$shell_cmd" == "bash" && "$script_path" == ci/scripts/*.sh ]]; then
    printf '%s' "$script_path"
  fi
}

validate_script_command() {
  local process="$1"
  local command_text="$2"
  local require_invoked="$3"
  local script_path
  script_path="$(script_from_command "$command_text")"

  if [[ -z "$script_path" ]]; then
    return 1
  fi
  if [[ ! -f "$script_path" ]]; then
    echo "evidence references missing script: $script_path (row: $process)" >&2
    exit 1
  fi
  if [[ ! -x "$script_path" ]]; then
    echo "evidence references non-executable script: $script_path (row: $process)" >&2
    exit 1
  fi
  if [[ "$require_invoked" == "yes" && "$script_path" != "$coverage_gate" && -z "${invoked_gate_scripts[$script_path]:-}" ]]; then
    echo "coverage gate missing required script invocation: $script_path (row: $process)" >&2
    exit 1
  fi
  return 0
}

validate_coverage_map() {
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
      validate_script_command "$process" "bash $automated_test" yes
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
}

validate_acceptance_evidence() {
  declare -A acceptance_counts=()
  declare -A acceptance_statuses=()
  declare -A acceptance_commands=()
  declare -A acceptance_tests=()
  local manual_required_count=0

  while IFS= read -r line; do
    [[ ! "$line" == "|"* ]] && continue

    if [[ "$line" =~ ^\|[[:space:]]*acceptance_id[[:space:]]*\|[[:space:]]*status[[:space:]]*\| ]]; then
      continue
    fi
    if [[ "$line" =~ ^\|[[:space:]]*-+[[:space:]]*\|[[:space:]]*-+[[:space:]]*\| ]]; then
      continue
    fi

    IFS='|' read -r _ acceptance_id status command_text test_id _notes _ <<< "$line"
    acceptance_id="$(trim "$acceptance_id")"
    status="$(trim "$status")"
    command_text="$(trim "$command_text")"
    test_id="$(trim "$test_id")"

    [[ -z "$acceptance_id" ]] && continue

    acceptance_counts["$acceptance_id"]=$(( ${acceptance_counts["$acceptance_id"]:-0} + 1 ))
    acceptance_statuses["$acceptance_id"]="$status"
    acceptance_commands["$acceptance_id"]="$command_text"
    acceptance_tests["$acceptance_id"]="$test_id"

    if [[ -z "$status" || -z "$command_text" || -z "$test_id" ]]; then
      echo "acceptance evidence has blank status, command, or test_id for: $acceptance_id" >&2
      exit 1
    fi
    if [[ "$status" != "automated" && "$status" != "manual-required" ]]; then
      echo "acceptance evidence has unsupported status for $acceptance_id: $status" >&2
      exit 1
    fi
    if [[ "$status" == "manual-required" ]]; then
      manual_required_count=$(( manual_required_count + 1 ))
    fi
  done < "$acceptance_evidence"

  for acceptance_id in "${required_acceptance_ids[@]}"; do
    count="${acceptance_counts[$acceptance_id]:-0}"
    if [[ "$count" -eq 0 ]]; then
      echo "acceptance evidence missing required row: $acceptance_id" >&2
      exit 1
    fi
    if [[ "$count" -ne 1 ]]; then
      echo "acceptance evidence must include required row exactly once: $acceptance_id (found $count)" >&2
      exit 1
    fi

    status="${acceptance_statuses[$acceptance_id]}"
    command_text="${acceptance_commands[$acceptance_id]}"
    test_id="${acceptance_tests[$acceptance_id]}"

    if validate_script_command "$acceptance_id" "$command_text" "$([[ "$status" == "automated" ]] && printf yes || printf no)"; then
      continue
    fi

    if [[ "$command_text" == cargo\ test* ]]; then
      if rust_test_exists "$test_id"; then
        continue
      fi
      echo "acceptance evidence references unresolved Rust test: $test_id (row: $acceptance_id)" >&2
      exit 1
    fi

    if [[ "$command_text" == ctest* || "$command_text" == cmake* ]]; then
      if cpp_test_exists "$test_id"; then
        continue
      fi
      echo "acceptance evidence references unresolved C++ test: $test_id (row: $acceptance_id)" >&2
      exit 1
    fi

    echo "acceptance evidence has unsupported command for $acceptance_id: $command_text" >&2
    exit 1
  done

  if (( manual_required_count > 0 )) && ! rg -q "V1 status: incomplete" "$acceptance_evidence"; then
    echo "acceptance evidence has manual-required rows but does not declare v1 incomplete" >&2
    exit 1
  fi
}

scan_for_suppressed_tests_and_lints
parse_coverage_gate
validate_coverage_map
validate_acceptance_evidence
