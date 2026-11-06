#!/usr/bin/env bash
# RingingV2 协议面直发测试(discovery → open → commands → SSE)。
# 用法:
#   ringing-probe.sh open                      # 开客户端会话,输出 CSID
#   ringing-probe.sh cmd <CSID> <command_id> '<command json>' [target_kind]
#   ringing-probe.sh sse <seed> [秒数] [since_cursor]   # 订阅事件流(默认 10s)
# 环境变量:QAQH_INSTANCE_ID(默认 ringing-probe)
set -euo pipefail

DISCOVERY="${XDG_CONFIG_HOME:-$HOME/.config}/qaqh/daemon.json"
INSTANCE="${QAQH_INSTANCE_ID:-ringing-probe}"

read_discovery() {
  python3 - "$DISCOVERY" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
print(d["endpoint"].split("/ringing")[0])
print(d["token"])
PY
}

BASE=""
TOKEN=""
{ read -r BASE; read -r TOKEN; } < <(read_discovery)
AUTH=("Authorization: Bearer $TOKEN")

case "${1:-}" in
open)
  curl -s -X POST "$BASE/ringing/v2/clients/open" -H "${AUTH[0]}" -H "Content-Type: application/json" \
    -d "{\"schema\":\"qaqh.Ringing\",\"version\":2,\"client_instance_id\":\"$INSTANCE\",\"command_contract_revision\":\"ringing-commands-7\",\"session_contract_revision\":\"ringing-session-2\"}"
  ;;
cmd)
  CSID="$2"; CMD_ID="$3"; COMMAND="$4"
  # 注意:不能用 ${5:-{...}} —— fallback 里的 } 会被 bash 当作 ${} 闭括号,
  # 恒给 TARGET 追加一个字面 },曾导致整条命令 400。
  TARGET='{"kind":"global"}'
  if (($# >= 5)); then TARGET="$5"; fi
  BODY=$(printf '{"schema":"qaqh.Ringing","version":2,"contract_revision":"ringing-commands-7","command_id":"%s","client_instance_id":"%s","client_session_id":"%s","target":%s,"preconditions":[],"command":%s}' "$CMD_ID" "$INSTANCE" "$CSID" "$TARGET" "$COMMAND")
  curl -s -X POST "$BASE/ringing/v2/commands" -H "${AUTH[0]}" -H "X-QAQH-Client-Session-Id: $CSID" -H "Content-Type: application/json" -d "$BODY"
  ;;
sse)
  SEED="$2"; CSID="$3"; SECONDS_LIMIT="${4:-10}"; SINCE="${5:-}"
  URL="$BASE/ringing/v2/sessions/$SEED/events"
  if [[ -n "$SINCE" ]]; then URL="$URL?since_cursor=$SINCE"; fi
  curl -s -N --max-time "$SECONDS_LIMIT" "$URL" -H "${AUTH[0]}" -H "X-QAQH-Client-Session-Id: $CSID" -H "Accept: text/event-stream" \
    | python3 -u -c '
import sys
frames, keepalive = 0, 0
buf = []
for raw in sys.stdin:
    line = raw.rstrip("\n")
    if line.startswith(":"):
        keepalive += 1
        continue
    if line.startswith("id:"):
        buf.append(line)
    elif line.startswith("event:") or line.startswith("data:"):
        buf.append(line)
    elif line == "" and buf:
        frames += 1
        d = next((x for x in buf if x.startswith("data:")), "")
        e = next((x for x in buf if x.startswith("event:") or x.startswith("id:")), "")
        head = d[5:200]
        print(f"frame#{frames} {e} {head}")
        buf = []
print(f"# 总帧 {frames},keepalive 注释行 {keepalive}", file=sys.stderr)
'
  ;;
*)
  echo "用法: ringing-probe.sh open | cmd <CSID> <command_id> '<command>' [target] | sse <seed> <CSID> [秒] [since]" >&2
  exit 2
  ;;
esac
