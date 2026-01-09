# Common cli tasks

tags:
	ctags -R --exclude=*/*.json --exclude=target/* .

lines:
	tokei

connect-mongodb:
    docker exec -it mongodb mongosh

# Run mailbox metrics demo against a local Prometheus OTLP receiver.
mailbox-metrics:
	cargo run -p capp --features observability --example mailbox_metrics

# Start Prometheus with OTLP receiver enabled (ports: web 9090, OTLP HTTP 4318, OTLP gRPC 4317).
prom-otlp:
	docker run --rm -p 9090:9090 prom/prometheus:latest --config.file=/etc/prometheus/prometheus.yml --enable-feature=otlp-write-receiver

# Emit a simple OTLP heartbeat to Prometheus (set OTEL_EXPORTER_OTLP_ENDPOINT if not using defaults).
otlp-heartbeat:
	cargo run -p capp --features observability --example otlp_heartbeat

# Start OTel Collector (OTLP 4318/4317 -> Prometheus exporter on 9090) using examples/otel-local.yaml.
otelcol-prom:
	docker run --rm --name otelcol-prom -p 4318:4318 -p 4317:4317 -p 9090:9090 -v $(pwd)/examples/otel-local.yaml:/etc/otelcol/config.yaml otel/opentelemetry-collector:latest --config /etc/otelcol/config.yaml
