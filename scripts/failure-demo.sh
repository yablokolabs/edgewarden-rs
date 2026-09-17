#!/usr/bin/env bash
# Failure-injection demo: control-plane outage, proxy crash, corrupt config,
# invalid policy, OTA health failure, duplicate/stale commands.
# Requires a built workspace (cargo build --workspace).
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

echo "=== 1. unit + integration tests (includes partition scenario) ==="
cargo test --workspace 2>&1 | grep -E "test result|FAILED" | head -n 25

echo ""
echo "=== 2. OTA state machine (success, corrupt, rollback, crash) ==="
cargo test -p edge-ota 2>&1 | grep -E "^test |test result" | head -n 20

echo ""
echo "=== 3. Canary rollout gates (bad rollout pauses) ==="
cargo test -p control-plane rollout 2>&1 | grep -E "^test |test result" | head -n 10

echo ""
echo "=== 4. Live partition demo: proxy keeps forwarding while cloud is down ==="
cargo test -p e2e --test partition -- --nocapture 2>&1 | tail -n 8

echo ""
echo "=== 5. Storm benchmark (small, local) ==="
# Start a throwaway echo upstream + proxy, then storm it.
cargo build -q -p edge-proxy -p storm
UPSTREAM_PORT=18881
PROXY_PORT=18880
python3 - <<PY &
import socket, threading
srv = socket.socket()
srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
srv.bind(("127.0.0.1", $UPSTREAM_PORT))
srv.listen(128)
def h(c):
    try:
        while True:
            d = c.recv(65536)
            if not d: break
            c.sendall(d)
    except OSError: pass
    finally: c.close()
while True:
    c, _ = srv.accept()
    threading.Thread(target=h, args=(c,), daemon=True).start()
PY
ECHO_PID=$!
./target/debug/edge-proxy --listen 127.0.0.1:$PROXY_PORT --upstream 127.0.0.1:$UPSTREAM_PORT --metrics-addr 127.0.0.1:19890 &
PROXY_PID=$!
sleep 1
./target/debug/storm --target 127.0.0.1:$PROXY_PORT --clients 50 --rounds 5 --payload-bytes 256 --out /tmp/storm-demo.json
cat /tmp/storm-demo.json
kill $PROXY_PID $ECHO_PID 2>/dev/null || true
echo ""
echo "failure-demo complete."
