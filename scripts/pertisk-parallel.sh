#!/usr/bin/env bash
# FIFO slot limiter for cluster VM creates. Source from *-create-cluster-vms.sh.
# Override: PERTISK_VM_JOBS=1 (serial) … 16. Default 4.
#
#   pertisk_parallel_init
#   pertisk_parallel_add "lab-cp-1" ./upload.sh --vmid 210 ...
#   pertisk_parallel_wait

pertisk_parallel_max() {
  local n="${PERTISK_VM_JOBS:-4}"
  if [[ ! "$n" =~ ^[1-9][0-9]*$ ]]; then
    n=4
  fi
  if (( n > 16 )); then
    n=16
  fi
  echo "$n"
}

pertisk_parallel_init() {
  _PERTISK_PAR_MAX="$(pertisk_parallel_max)"
  _PERTISK_PAR_DIR="$(mktemp -d "${TMPDIR:-/tmp}/pertisk-vm-jobs.XXXXXX")"
  _PERTISK_PAR_PIDS=()
  _PERTISK_PAR_LABELS=()
  _PERTISK_PAR_LOGS=()
  _PERTISK_PAR_FAIL=0
  echo "==> parallel VM create max=${_PERTISK_PAR_MAX} (PERTISK_VM_JOBS)"
}

_pertisk_par_reap_oldest() {
  local pid="${_PERTISK_PAR_PIDS[0]}"
  local label="${_PERTISK_PAR_LABELS[0]}"
  local log="${_PERTISK_PAR_LOGS[0]}"
  _PERTISK_PAR_PIDS=("${_PERTISK_PAR_PIDS[@]:1}")
  _PERTISK_PAR_LABELS=("${_PERTISK_PAR_LABELS[@]:1}")
  _PERTISK_PAR_LOGS=("${_PERTISK_PAR_LOGS[@]:1}")
  local st=0
  wait "$pid" || st=$?
  echo "==> ${label} finished (exit ${st})"
  if [[ -f "$log" ]]; then
    cat "$log"
  fi
  if [[ "$st" -ne 0 ]]; then
    echo "ERROR: ${label} failed (exit ${st})" >&2
    _PERTISK_PAR_FAIL=1
    return 1
  fi
  return 0
}

pertisk_parallel_add() {
  local label="$1"
  shift
  while (( ${#_PERTISK_PAR_PIDS[@]} >= _PERTISK_PAR_MAX )); do
    _pertisk_par_reap_oldest || true
  done
  if [[ "${_PERTISK_PAR_FAIL}" -ne 0 ]]; then
    echo "==> skip ${label} (earlier VM create failed)"
    return 0
  fi
  local safe
  safe="$(printf '%s' "$label" | tr -c 'A-Za-z0-9._-' '_')"
  local log="${_PERTISK_PAR_DIR}/${safe}.log"
  echo "==> start ${label}"
  "$@" >"$log" 2>&1 &
  _PERTISK_PAR_PIDS+=("$!")
  _PERTISK_PAR_LABELS+=("$label")
  _PERTISK_PAR_LOGS+=("$log")
}

pertisk_parallel_wait() {
  while (( ${#_PERTISK_PAR_PIDS[@]} > 0 )); do
    _pertisk_par_reap_oldest || true
  done
  rm -rf "${_PERTISK_PAR_DIR:-}"
  if [[ "${_PERTISK_PAR_FAIL}" -ne 0 ]]; then
    echo "ERROR: one or more VM creates failed" >&2
    return 1
  fi
  return 0
}
