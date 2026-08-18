#!/bin/sh -e
# Bridge end-to-end: LTFR datagram in, sysfs-format write out, and what a
# second reader of the same telemetry gets (fed by the first, and promoted
# when the first exits).
#
# On an isolated port (LOGI_RPM_PORT), like test_bridge_leds.py: the standard
# 20780 may already be held by a live bridge or by logi-tf-sim, and this test
# needs to be the only thing arranging the ports it uses.
cd "$(dirname "$0")"
PORT=21880
export PORT
cc -O2 -Wall -o /tmp/logi-rpm-bridge ../../tools/logi-rpm-bridge.c
out=$(mktemp)
: > "$out"
LOGI_RPM_SYSFS="$out" LOGI_RPM_PORT="$PORT" /tmp/logi-rpm-bridge & bpid=$!
trap 'kill "$bpid" 2>/dev/null || true' EXIT
sleep 0.3
python3 - <<'PY'
import os, socket, struct
pkt = bytearray(28)
pkt[0:4] = b"LTFR"; pkt[4] = 2
pkt[14:18] = struct.pack('<f', 6543.2)
pkt[18:22] = struct.pack('<f', 14000.0)
socket.socket(socket.AF_INET, socket.SOCK_DGRAM).sendto(
    bytes(pkt), ("127.0.0.1", int(os.environ["PORT"])))
PY
sleep 0.3

# A second reader of the same telemetry. The kernel will not give one
# unicast datagram to two sockets, so the one that holds the port forwards
# to the fan-out ports behind it: the second reader must end up with the
# same sample rather than an explanation of why it cannot have it.
grep -q "^6543 14000$" "$out" || { echo "FAIL: got '$(cat "$out")'"; exit 1; }
out2=$(mktemp)
: > "$out2"
LOGI_RPM_SYSFS="$out2" LOGI_RPM_PORT="$PORT" /tmp/logi-rpm-bridge 2>/tmp/logi-bridge-follower.log & fpid=$!
trap 'kill "$bpid" "$fpid" 2>/dev/null || true' EXIT
sleep 0.4
grep -q "fan-out" /tmp/logi-bridge-follower.log || {
	echo "FAIL: the second bridge did not say it was following:"
	cat /tmp/logi-bridge-follower.log; exit 1; }
send_to_port() {
	RPM="$1" PORT="$PORT" python3 - <<'PY'
import os, socket, struct
pkt = bytearray(28)
pkt[0:4] = b"LTFR"; pkt[4] = 2
pkt[14:18] = struct.pack('<f', float(os.environ["RPM"]))
pkt[18:22] = struct.pack('<f', 14000.0)
socket.socket(socket.AF_INET, socket.SOCK_DGRAM).sendto(
    bytes(pkt), ("127.0.0.1", int(os.environ["PORT"])))
PY
}
send_to_port 5200
sleep 0.4
grep -q "^5200 14000$" "$out" || { echo "FAIL: the holder lost its own datagram: '$(cat "$out")'"; exit 1; }
grep -q "^5200 14000$" "$out2" || { echo "FAIL: the follower was not fed: '$(cat "$out2")'"; exit 1; }

# And when the holder exits, the survivor takes the port back, so telemetry
# aimed where every producer sends it keeps arriving.
kill $bpid 2>/dev/null; wait $bpid 2>/dev/null || true
sleep 3
send_to_port 4100
sleep 0.4
grep -q "^4100 14000$" "$out2" || {
	echo "FAIL: the survivor did not take the port back: '$(cat "$out2")'"; exit 1; }
kill $fpid 2>/dev/null; wait $fpid 2>/dev/null || true

# A wheel that goes away and comes back. The bridge resolves its attribute
# path once at startup and used to hold it for the process lifetime, which
# makes that path an identity only until the first replug: hidraw numbering
# is recycled, so the node it was started against can come back as a
# different wheel's. It now looks again when a write says the path is gone,
# and must survive the gap without dying or wedging.
#
# The gap is made by removing the attribute's DIRECTORY, not the file: the
# file alone would simply be recreated by the next write, which is not what
# a departed wheel looks like.
dir=$(mktemp -d)
attr="$dir/wheel_texture_rpm"
LOGI_RPM_SYSFS="$attr" LOGI_RPM_PORT="$PORT" /tmp/logi-rpm-bridge 2>/dev/null & bpid=$!
trap 'kill "$bpid" 2>/dev/null || true' EXIT
sleep 0.3
send_rpm() {
	RPM="$1" python3 - <<'PY'
import os, socket, struct
pkt = bytearray(28)
pkt[0:4] = b"LTFR"; pkt[4] = 2
pkt[14:18] = struct.pack('<f', float(os.environ["RPM"]))
pkt[18:22] = struct.pack('<f', 14000.0)
socket.socket(socket.AF_INET, socket.SOCK_DGRAM).sendto(
    bytes(pkt), ("127.0.0.1", int(os.environ["PORT"])))
PY
	sleep 0.3
}
send_rpm 1000
grep -q "^1000 14000$" "$attr" || { echo "FAIL: no first write: '$(cat "$attr" 2>&1)'"; exit 1; }
rm -rf "$dir"
send_rpm 2000
kill -0 "$bpid" 2>/dev/null || { echo "FAIL: the bridge died when the wheel went away"; exit 1; }
mkdir -p "$dir"
send_rpm 3000
grep -q "^3000 14000$" "$attr" || { echo "FAIL: no write after the wheel came back: '$(cat "$attr" 2>&1)'"; exit 1; }
kill $bpid 2>/dev/null; wait $bpid 2>/dev/null || true
rm -rf "$dir"
echo "bridge test pass"
