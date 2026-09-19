#!/bin/sh
set -eu
# The Worker authenticates Google-issued relay JWTs. This process is not on
# the public internet; it only accepts connections the Worker forwards.
cat > /tmp/relay.toml <<EOF
enable_relay = true
http_bind_addr = "0.0.0.0:8080"
enable_quic_addr_discovery = false
enable_metrics = true
metrics_bind_addr = "0.0.0.0:9090"
access = "everyone"
[limits.client_rx]
bytes_per_second = 4194304
max_burst_bytes = 8388608
EOF
exec /usr/local/bin/iroh-relay --config-path /tmp/relay.toml
