#!/bin/sh -e
# Bridge end-to-end: LTFR datagram in, sysfs-format write out.
cd "$(dirname "$0")"
cc -O2 -Wall -o /tmp/logi-rpm-bridge ../../tools/logi-rpm-bridge.c
out=$(mktemp)
: > "$out"
LOGI_RPM_SYSFS="$out" /tmp/logi-rpm-bridge & bpid=$!
sleep 0.3
python3 - <<'PY'
import socket, struct
pkt = bytearray(28)
pkt[0:4] = b"LTFR"; pkt[4] = 2
pkt[14:18] = struct.pack('<f', 6543.2)
pkt[18:22] = struct.pack('<f', 14000.0)
socket.socket(socket.AF_INET, socket.SOCK_DGRAM).sendto(bytes(pkt), ("127.0.0.1", 20780))
PY
sleep 0.3
kill $bpid 2>/dev/null; wait $bpid 2>/dev/null || true
grep -q "^6543 14000$" "$out" || { echo "FAIL: got '$(cat "$out")'"; exit 1; }
echo "bridge test pass"
