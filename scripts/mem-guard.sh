#!/usr/bin/env bash
# 内存守卫:可用内存低于阈值时等待,回升后才放行命令。
# 用法:mem-guard.sh [阈值MB] <命令...>;阈值默认 1100MB。
# 背景:本机 8G 内存,常驻两个 ZCode 实例;gpui 依赖树即使 -j1 也有单包峰值,
# 低水位强编会挤爆 swap 并卡死前台会话。
set -euo pipefail

THRESHOLD_MB="${1:-1100}"
if [[ "${1:-}" =~ ^[0-9]+$ ]]; then shift; fi

avail_mb() { LC_ALL=C free -m | awk '/^Mem:/{print $7}'; }

while :; do
  avail="$(avail_mb)"
  if (( avail >= THRESHOLD_MB )); then
    break
  fi
  echo "[mem-guard] 可用 ${avail}MB < ${THRESHOLD_MB}MB,60s 后重试 $(date +%H:%M:%S)" >&2
  sleep 60
done

sync
echo "[mem-guard] 可用 $(avail_mb)MB,放行:$*" >&2
exec "$@"
