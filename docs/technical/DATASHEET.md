# CoucheStor - Technical Datasheet

## Product Overview

The CoucheStor is a Kubernetes-native solution for automated storage tiering in OpenEBS Mayastor environments. It intelligently migrates volumes between high-performance NVMe and cost-effective SATA storage tiers based on real-time performance metrics.

---

## Key Specifications

### General

| Specification | Value |
|--------------|-------|
| Product Name | CoucheStor |
| Version | 1.0.0 |
| License | Apache 2.0 |
| Language | Rust |
| Binary Size | ~7 MB |
| Container Image | `ghcr.io/billyronks/couchestor:v1.0.0` |

### Compatibility

| Component | Minimum Version | Recommended Version |
|-----------|----------------|---------------------|
| Kubernetes | 1.25 | 1.28+ |
| OpenEBS Mayastor | 2.0 | 2.4+ |
| Prometheus | 2.30 | 2.45+ |
| Container Runtime | Any OCI-compliant | containerd 1.6+ |

### Resource Requirements

| Resource | Request | Limit |
|----------|---------|-------|
| CPU | 100m | 500m |
| Memory | 128Mi | 256Mi |
| Disk | None | None |

---

## Features

### Core Features

| Feature | Description |
|---------|-------------|
| Automated Tiering | Automatically migrates volumes based on IOPS thresholds |
| Policy-Based | Declarative CRD-based configuration |
| Safe Migration | 4-phase process ensures zero data loss |
| Metrics-Driven | Uses Prometheus for performance analysis |
| Multi-Policy | Support for multiple independent policies |

### Operational Features

| Feature | Description |
|---------|-------------|
| Dry-Run Mode | Validate behavior without making changes |
| Preservation Mode | Keep old replicas after migration |
| Configurable Timeouts | Adjustable sync and migration timeouts |
| Concurrent Migration Limits | Control parallel migration count |
| Cooldown Periods | Prevent migration thrashing |

### Observability Features

| Feature | Description |
|---------|-------------|
| Prometheus Metrics | Native metrics endpoint |
| Structured Logging | JSON or text logging |
| Health Endpoints | Liveness and readiness probes |
| Status Reporting | Real-time policy status in CRD |
| Migration History | Recent migration audit trail |

---

## Architecture

### Components

| Component | Function |
|-----------|----------|
| MetricsWatcher | Queries Prometheus for volume IOPS |
| Controller | Reconciles StoragePolicy resources |
| Migrator | Executes safe volume migrations |
| Health Server | Provides health check endpoints |
| Metrics Server | Exposes Prometheus metrics |

### Integration Points

| System | Protocol | Port | Direction |
|--------|----------|------|-----------|
| Kubernetes API | HTTPS | 443/6443 | Outbound |
| Prometheus | HTTP | 9090 | Outbound |
| Metrics Scrape | HTTP | 8080 | Inbound |
| Health Probes | HTTP | 8081 | Inbound |

---

## Configuration

### StoragePolicy CRD

```yaml
apiVersion: storage.billyronks.io/v1
kind: StoragePolicy
metadata:
  name: example
spec:
  highWatermarkIOPS: 5000      # Migrate to NVMe above this
  lowWatermarkIOPS: 500        # Migrate to SATA below this
  samplingWindow: "1h"         # IOPS averaging window
  cooldownPeriod: "24h"        # Minimum time between migrations
  storageClassName: "mayastor" # Target StorageClass
  nvmePoolSelector:            # Hot tier pool selector
    matchLabels:
      tier: hot
  sataPoolSelector:            # Cold tier pool selector
    matchLabels:
      tier: cold
  maxConcurrentMigrations: 2   # Parallel migration limit
  migrationTimeout: "30m"      # Single migration timeout
  enabled: true                # Policy active
  dryRun: false               # Log-only mode
```

### Operator Configuration

| Parameter | Default | Description |
|-----------|---------|-------------|
| `--prometheus-url` | `http://prometheus.monitoring.svc.cluster.local:9090` | Prometheus server URL |
| `--max-concurrent-migrations` | 2 | Maximum parallel migrations |
| `--migration-timeout-minutes` | 30 | Migration timeout |
| `--sync-poll-interval-seconds` | 10 | Replica sync check interval |
| `--dry-run` | false | Global dry-run mode |
| `--preservation-mode` | false | Keep old replicas |
| `--log-level` | info | Logging verbosity |
| `--log-json` | false | JSON log format |

---

## Performance

### Benchmarks

| Metric | Value | Conditions |
|--------|-------|------------|
| Reconciliation Time | < 30s | 100 volumes |
| Prometheus Query | < 5s | Standard deployment |
| Memory (Steady State) | ~50 MB | 100 volumes, cache enabled |
| CPU (Idle) | < 10m | Between reconciliations |

### Scalability

| Dimension | Tested Limit | Notes |
|-----------|-------------|-------|
| Volumes per Cluster | 1,000+ | Paginated list operations |
| Policies per Cluster | 100+ | Independent reconciliation |
| Concurrent Migrations | 10 | Configurable |
| Migration Rate | 100/day | Depends on sync time |

---

## Security

### Security Features

| Feature | Implementation |
|---------|---------------|
| RBAC | Least-privilege ClusterRole |
| Non-Root | Runs as UID 65534 |
| Read-Only FS | No writable filesystem |
| No Capabilities | All Linux capabilities dropped |
| No Secrets | No sensitive data handling |

### Compliance Support

| Standard | Status |
|----------|--------|
| SOC 2 Type II | Supported |
| ISO 27001 | Supported |
| PCI DSS | Supported |
| HIPAA | Supported |

---

## Metrics Exposed

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `storage_operator_reconcile_total` | Counter | - | Total reconciliations |
| `storage_operator_migrations_total` | Counter | status | Migrations by status |
| `storage_operator_active_migrations` | Gauge | - | Current in-progress |

---

## Endpoints

### Health Server (Port 8081)

| Endpoint | Response | Purpose |
|----------|----------|---------|
| `/healthz` | 200 OK | Liveness probe |
| `/livez` | 200 OK | Liveness probe |
| `/readyz` | 200 OK | Readiness probe |

### Metrics Server (Port 8080)

| Endpoint | Response | Purpose |
|----------|----------|---------|
| `/metrics` | Prometheus format | Metrics exposition |

---

## Support

### Documentation

| Resource | URL |
|----------|-----|
| GitHub Repository | https://github.com/billyronks/couchestor |
| Documentation | https://docs.billyronks.io/couchestor |
| API Reference | https://docs.billyronks.io/couchestor/api |

### Support Channels

| Channel | Response Time |
|---------|--------------|
| GitHub Issues | 48 hours |
| Slack Community | Best effort |
| Enterprise Support | 4 hours (critical) |

---

## Version History

| Version | Release Date | Notes |
|---------|--------------|-------|
| 1.0.0 | 2026-02-02 | Initial release |

---

*This datasheet is subject to change. Refer to the official documentation for the most current specifications.*
