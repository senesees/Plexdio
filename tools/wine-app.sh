#!/usr/bin/env bash
# Helpers for driving plexdio.exe under Wine during development.
#   tools/wine-app.sh pid            -> print the PID of the running plexdio.exe
#   tools/wine-app.sh stats [secs]   -> RSS (MB) and CPU% averaged over [secs] (default 30)
#   tools/wine-app.sh kill           -> terminate it
set -euo pipefail
pid() { ps -eo pid,args | awk '/[Z]:.*[p]lexdio\.exe/ && !/wine-app|run-windows|winedbg/ {print $1; exit}'; }
case "${1:-pid}" in
  pid) pid ;;
  stats)
    p=$(pid); [ -n "$p" ] || { echo "not running"; exit 1; }
    secs=${2:-30}; hz=$(getconf CLK_TCK)
    t0=$(awk '{print $14+$15}' /proc/$p/stat); sleep "$secs"; t1=$(awk '{print $14+$15}' /proc/$p/stat)
    rss=$(awk '/VmRSS/ {print $2}' /proc/$p/status)
    echo "pid=$p rss_mb=$((rss/1024)) cpu_pct=$(awk -v a=$t0 -v b=$t1 -v h=$hz -v s=$secs 'BEGIN{printf "%.2f", (b-a)*100/h/s}')" ;;
  kill) p=$(pid); [ -n "$p" ] && kill "$p" || true ;;
esac
