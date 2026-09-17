#!/usr/bin/env bash
# Generate development PKI for mTLS (dev/demo ONLY).
# Production provisioning must use a real CA / SPIRE / cloud IoT PKI with
# per-device enrollment, short-lived certs, and HSM-backed keys.
# See docs/security.md. Never commit output of this script.
set -euo pipefail
OUT="${1:-certs}"
mkdir -p "$OUT"
echo "[gen-certs] writing dev CA + server + 3 device certs to $OUT/"

openssl req -x509 -newkey rsa:2048 -nodes -days 7 \
  -keyout "$OUT/ca.key" -out "$OUT/ca.crt" \
  -subj "/CN=edgewarden-dev-ca" >/dev/null 2>&1

openssl req -newkey rsa:2048 -nodes \
  -keyout "$OUT/server.key" -out "$OUT/server.csr" \
  -subj "/CN=localhost" >/dev/null 2>&1
openssl x509 -req -in "$OUT/server.csr" -CA "$OUT/ca.crt" -CAkey "$OUT/ca.key" \
  -CAcreateserial -days 7 -out "$OUT/server.crt" >/dev/null 2>&1

for d in edge-01 edge-02 edge-03; do
  openssl req -newkey rsa:2048 -nodes \
    -keyout "$OUT/$d.key" -out "$OUT/$d.csr" \
    -subj "/CN=$d" >/dev/null 2>&1
  openssl x509 -req -in "$OUT/$d.csr" -CA "$OUT/ca.crt" -CAkey "$OUT/ca.key" \
    -CAcreateserial -days 7 -out "$OUT/$d.crt" >/dev/null 2>&1
  rm -f "$OUT/$d.csr"
done
rm -f "$OUT/server.csr" "$OUT/ca.srl"
chmod 600 "$OUT"/*.key
echo "[gen-certs] done. DO NOT COMMIT $OUT/ (gitignored)."
ls -l "$OUT"
