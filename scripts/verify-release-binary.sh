#!/usr/bin/env bash
#
# verify-release-binary.sh — assert a release binary is what we claim it is.
#
# We ship two flavours of Linux binary (see .github/workflows/release.yml):
#   - musl: fully static, runs on any Linux regardless of installed libc.
#   - gnu : dynamically linked against glibc. Built with `cross`, whose images
#           use an old glibc (Ubuntu 20.04 = glibc 2.31) so the binary stays
#           runnable on older distros. A naive `cargo build` on a modern runner
#           would instead bake in that runner's (much newer) glibc symbols and
#           silently produce a binary that won't start on older systems.
#
# This script is the guard against that failure mode. It is run both locally
# (TDD loop) and from CI after each build, over every produced binary.
#
# Usage:
#   verify-release-binary.sh --libc <gnu|musl> [--glibc-floor X.Y] [--no-run] <binary>
#
# Checks performed:
#   1. linkage  — gnu must be dynamically linked (has a program interpreter);
#                 musl must be statically linked.
#   2. run      — the binary executes `--help` and exits 0. Skipped when the
#                 binary's architecture differs from the host's (e.g. an
#                 aarch64 artifact verified on an x86_64 CI runner): we can
#                 check its shape but not run it without an emulator.
#   3. glibc floor (gnu only) — the highest GLIBC_x.y symbol version the binary
#                 requires must not exceed the floor, i.e. the binary must run
#                 on a system whose glibc is as old as the floor. This is the
#                 portability guarantee that justifies building with `cross`.
#
set -euo pipefail

# cross's *-unknown-linux-gnu images are Ubuntu 20.04 based → glibc 2.31.
DEFAULT_GLIBC_FLOOR="2.31"

# assert_glibc_floor MAX FLOOR — the portability policy for the gnu binaries.
#
# Given MAX (the highest GLIBC_x.y[.z] version a binary requires, e.g. "2.34")
# and FLOOR (the oldest glibc we promise to run on, e.g. "2.31"), pass (return
# 0) iff the binary can run on a glibc-FLOOR system, i.e. MAX <= FLOOR. A binary
# that needs newer symbols than the floor is a hard failure: that is the whole
# point of building the gnu flavour with `cross` rather than a bare `cargo
# build`, so a regression here must fail the release, not merely warn.
#
# Dotted versions do not compare as plain numbers (2.9 < 2.31), so we lean on
# `sort -V`: it orders them correctly, putting the lower of {MAX, FLOOR} first.
# MAX is acceptable exactly when it is that lower value (or equal to FLOOR).
assert_glibc_floor() {
  local max="$1" floor="$2"
  [[ "$(printf '%s\n%s\n' "$max" "$floor" | sort -V | head -1)" == "$max" ]]
}

libc=""
glibc_floor="$DEFAULT_GLIBC_FLOOR"
run_check=1
bin=""

die() { echo "error: $*" >&2; exit 2; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --libc)        libc="${2:-}"; shift 2 ;;
    --glibc-floor) glibc_floor="${2:-}"; shift 2 ;;
    --no-run)      run_check=0; shift ;;
    -h|--help)     sed -n '2,33p' "$0"; exit 0 ;;
    -*)            die "unknown flag: $1" ;;
    *)             bin="$1"; shift ;;
  esac
done

[[ -n "$libc" ]] || die "--libc <gnu|musl> is required"
[[ "$libc" == "gnu" || "$libc" == "musl" ]] || die "--libc must be 'gnu' or 'musl', got '$libc'"
[[ -n "$bin" ]] || die "no binary given"
[[ -f "$bin" ]] || die "no such file: $bin"

# --- architecture detection --------------------------------------------------
# Map the ELF machine and `uname -m` onto a common arch token so we can tell
# whether the binary can be executed on this host.
elf_machine="$(readelf -h "$bin" | awk -F: '/Machine:/ { gsub(/^[ \t]+/, "", $2); print $2 }')"
case "$elf_machine" in
  *X86-64*)  bin_arch="x86_64" ;;
  *AArch64*) bin_arch="aarch64" ;;
  *)         bin_arch="unknown" ;;
esac
case "$(uname -m)" in
  x86_64)          host_arch="x86_64" ;;
  aarch64|arm64)   host_arch="aarch64" ;;
  *)               host_arch="unknown" ;;
esac

fail=0
note() { printf '  %s %s\n' "$1" "$2"; }
pass() { note "PASS" "$1"; }
bad()  { note "FAIL" "$1"; fail=1; }

echo "verifying $bin (--libc $libc, arch $bin_arch)"

# --- check 1: linkage --------------------------------------------------------
file_out="$(file "$bin")"
if [[ "$libc" == "musl" ]]; then
  if [[ "$file_out" == *"statically linked"* ]]; then
    pass "statically linked (musl)"
  else
    bad "expected a statically linked binary, got: $file_out"
  fi
else # gnu
  if [[ "$file_out" == *"dynamically linked"* && "$file_out" == *"interpreter"* ]]; then
    pass "dynamically linked against glibc"
  else
    bad "expected a dynamically linked glibc binary, got: $file_out"
  fi
fi

# --- check 2: runs -----------------------------------------------------------
if [[ "$run_check" -eq 0 ]]; then
  note "SKIP" "run check disabled (--no-run)"
elif [[ "$bin_arch" != "$host_arch" || "$bin_arch" == "unknown" ]]; then
  note "SKIP" "run check: binary is $bin_arch, host is $host_arch (cannot execute natively)"
else
  if "$bin" --help >/dev/null 2>&1; then
    pass "executes (--help exits 0)"
  else
    bad "binary did not run cleanly (--help exit $?)"
  fi
fi

# --- check 3: glibc floor (gnu only) ----------------------------------------
# Highest GLIBC_x.y[.z] version referenced in the binary's version-needs table.
# `sort -V` understands dotted version order; tail -1 takes the maximum.
max_glibc="$(readelf -V "$bin" 2>/dev/null \
  | grep -oE 'GLIBC_[0-9]+\.[0-9]+(\.[0-9]+)?' \
  | sed 's/GLIBC_//' \
  | sort -uV | tail -1 || true)"

if [[ "$libc" == "gnu" ]]; then
  if [[ -z "$max_glibc" ]]; then
    bad "could not read any GLIBC symbol versions from $bin"
  elif assert_glibc_floor "$max_glibc" "$glibc_floor"; then
    pass "max required glibc ($max_glibc) is within floor ($glibc_floor)"
  else
    bad "binary requires glibc $max_glibc but the floor is $glibc_floor (not portable to older systems)"
  fi
fi

if [[ "$fail" -ne 0 ]]; then
  echo "VERIFY FAILED: $bin"
  exit 1
fi
echo "VERIFY OK: $bin"
