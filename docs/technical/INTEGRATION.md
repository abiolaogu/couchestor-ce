# Integration Guide

## Document Information

| Field | Value |
|-------|-------|
| Version | 1.0.0 |
| Author | BillyRonks Engineering |
| Last Updated | 2026-02-02 |

---

## 1. Prometheus Integration

### 1.1 Scrape Configuration

Add the CoucheStor to your Prometheus scrape configuration:

```yaml
# prometheus.yml
scrape_configs:
  - job_name: 'couchestor'
    kubernetes_sd_configs:
      - role: pod
        namespaces:
          names:
            - kube-system
    relabel_configs:
      - source_labels: [__meta_kubernetes_pod_label_app]
        action: keep
        regex: couchestor
      - source_labels: [__meta_kubernetes_pod_container_port_number]
        action: keep
        regex: "8080"
```

Or using ServiceMonitor (Prometheus Operator):

```yaml
apiVersion: monitoring.coreos.com/v1
kind: ServiceMonitor
metadata:
  name: couchestor
  namespace: monitoring
spec:
  selector:
    matchLabels:
      app: couchestor
  namespaceSelector:
    matchNames:
      - kube-system
  endpoints:
    - port: metrics
      interval: 30s
```

### 1.2 Required Mayastor Metrics

Ensure Prometheus is scraping these Mayastor metrics:

```yaml
# Mayastor ServiceMonitor
apiVersion: monitoring.coreos.com/v1
kind: ServiceMonitor
metadata:
  name: mayastor
  namespace: monitoring
spec:
  selector:
    matchLabels:
      app: mayastor
  namespaceSelector:
    matchNames:
      - mayastor
  endpoints:
    - port: metrics
```

Required metrics:
- `openebs_volume_iops` (or `mayastor_volume_iops`)
- Labels: `volume_id`

### 1.3 Verify Metrics Availability

```bash
# Check if metrics exist
curl -s "http://prometheus:9090/api/v1/query?query=openebs_volume_iops" | jq '.data.result | length'

# Query specific volume
curl -s "http://prometheus:9090/api/v1/query?query=openebs_volume_iops{volume_id=\"pvc-abc123\"}"
```

---

## 2. Grafana Integration

### 2.1 Dashboard Import

Import the provided dashboard JSON:

```bash
# Download dashboard
curl -O https://raw.githubusercontent.com/billyronks/couchestor/main/dashboards/couchestor.json

# Import via Grafana API
curl -X POST \
  -H "Content-Type: application/json" \
  -d @couchestor.json \
  http://admin:admin@grafana:3000/api/dashboards/db
```

### 2.2 Dashboard Panels

The dashboard includes:

| Panel | Description |
|-------|-------------|
| Policy Overview | Status of all policies |
| Migration Activity | Migrations over time |
| Volume Distribution | Hot vs Cold tier volumes |
| Heat Score Histogram | Distribution of volume activity |
| Recent Migrations | Table of recent migrations |
| Operator Health | CPU, memory, reconciliation time |

### 2.3 Sample Grafana Queries

**Migrations per Hour:**
```promql
sum(rate(storage_operator_migrations_total[1h])) by (status)
```

**Active Migrations:**
```promql
storage_operator_active_migrations
```

**Volume Distribution:**
```promql
# Requires custom metric or status scraping
# Placeholder: use policy status API
```

---

## 3. AlertManager Integration

### 3.1 Alert Rules

```yaml
# prometheus-rules.yaml
apiVersion: monitoring.coreos.com/v1
kind: PrometheusRule
metadata:
  name: couchestor-alerts
  namespace: monitoring
spec:
  groups:
    - name: couchestor
      rules:
        - alert: StorageOperatorDown
          expr: up{job="couchestor"} == 0
          for: 5m
          labels:
            severity: critical
          annotations:
            summary: "CoucheStor is down"
            description: "The operator has been unreachable for 5 minutes."

        - alert: MigrationFailureSpike
          expr: increase(storage_operator_migrations_total{status="failed"}[1h]) > 5
          for: 10m
          labels:
            severity: warning
          annotations:
            summary: "Multiple migration failures detected"
            description: "{{ $value }} migrations failed in the last hour."

        - alert: MigrationBacklog
          expr: storage_operator_active_migrations >= 0.8 * storage_operator_max_concurrent_migrations
          for: 30m
          labels:
            severity: warning
          annotations:
            summary: "Migration queue near capacity"
            description: "Active migrations at {{ $value | humanize }}% of limit."
```

### 3.2 Notification Templates

```yaml
# alertmanager.yml
templates:
  - '/etc/alertmanager/templates/storage-operator.tmpl'

route:
  receiver: 'storage-team'
  routes:
    - match:
        job: couchestor
      receiver: 'storage-team'
```

---

## 4. Logging Integration

### 4.1 Fluentd/Fluent Bit

Configure log collection:

```yaml
# fluent-bit ConfigMap
[INPUT]
    Name              tail
    Tag               kube.*
    Path              /var/log/containers/couchestor*.log
    Parser            docker
    Mem_Buf_Limit     5MB
    Skip_Long_Lines   On

[FILTER]
    Name              kubernetes
    Match             kube.*
    Merge_Log         On
    K8S-Logging.Parser On

[OUTPUT]
    Name              es
    Match             *
    Host              elasticsearch
    Port              9200
    Index             kubernetes-logs
```

### 4.2 JSON Logging

Enable JSON logging for better parsing:

```yaml
# deployment.yaml
env:
  - name: LOG_JSON
    value: "true"
```

JSON log format:
```json
{
  "timestamp": "2026-02-02T10:30:00.000Z",
  "level": "INFO",
  "target": "couchestor::controller",
  "message": "Migration completed",
  "volume": "pvc-abc123",
  "duration_ms": 125000
}
```

### 4.3 Log Queries (Elasticsearch/Kibana)

```
# Find all migrations
kubernetes.labels.app:"couchestor" AND message:"migration"

# Find errors
kubernetes.labels.app:"couchestor" AND level:"ERROR"

# Find specific volume
kubernetes.labels.app:"couchestor" AND volume:"pvc-abc123"
```

---

## 5. GitOps Integration

### 5.1 Flux CD

```yaml
# flux-system/couchestor.yaml
apiVersion: source.toolkit.fluxcd.io/v1beta2
kind: GitRepository
metadata:
  name: couchestor
  namespace: flux-system
spec:
  interval: 1m
  url: https://github.com/your-org/infrastructure
  ref:
    branch: main
---
apiVersion: kustomize.toolkit.fluxcd.io/v1beta2
kind: Kustomization
metadata:
  name: couchestor
  namespace: flux-system
spec:
  interval: 10m
  sourceRef:
    kind: GitRepository
    name: couchestor
  path: ./clusters/production/couchestor
  prune: true
```

### 5.2 ArgoCD

```yaml
# argocd/couchestor.yaml
apiVersion: argoproj.io/v1alpha1
kind: Application
metadata:
  name: couchestor
  namespace: argocd
spec:
  project: default
  source:
    repoURL: https://github.com/your-org/infrastructure
    targetRevision: HEAD
    path: clusters/production/couchestor
  destination:
    server: https://kubernetes.default.svc
    namespace: kube-system
  syncPolicy:
    automated:
      prune: true
      selfHeal: true
```

---

## 6. CI/CD Integration

### 6.1 Policy Validation

Add policy validation to your CI pipeline:

```yaml
# .github/workflows/validate.yaml
jobs:
  validate-policies:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3

      - name: Install kubectl
        uses: azure/setup-kubectl@v3

      - name: Validate StoragePolicy YAML
        run: |
          kubectl apply --dry-run=client -f policies/

      - name: Check policy syntax
        run: |
          for f in policies/*.yaml; do
            yq eval '.spec.highWatermarkIOPS > .spec.lowWatermarkIOPS' $f
          done
```

### 6.2 Integration Testing

```yaml
# Test policy in staging
- name: Apply policy to staging
  run: |
    kubectl --context staging apply -f policies/test-policy.yaml

- name: Wait for reconciliation
  run: |
    sleep 60
    kubectl --context staging get storagepolicy test-policy -o jsonpath='{.status.phase}'

- name: Verify policy is active
  run: |
    phase=$(kubectl --context staging get storagepolicy test-policy -o jsonpath='{.status.phase}')
    [ "$phase" = "Active" ] || exit 1
```

---

## 7. Service Mesh Integration

### 7.1 Istio

The operator makes outbound calls to:
- Kubernetes API (port 443/6443)
- Prometheus (port 9090)

```yaml
# VirtualService for operator metrics (optional)
apiVersion: networking.istio.io/v1beta1
kind: VirtualService
metadata:
  name: couchestor
spec:
  hosts:
    - couchestor.kube-system.svc.cluster.local
  http:
    - match:
        - uri:
            prefix: /metrics
      route:
        - destination:
            host: couchestor.kube-system.svc.cluster.local
            port:
              number: 8080
```

### 7.2 Network Policies

See [Security Documentation](../enterprise/SECURITY.md) for NetworkPolicy examples.

---

## 8. External Systems

### 8.1 Slack Notifications

Using AlertManager Slack integration:

```yaml
# alertmanager.yml
receivers:
  - name: 'storage-slack'
    slack_configs:
      - api_url: 'https://hooks.slack.com/services/xxx'
        channel: '#storage-alerts'
        title: 'Storage Operator Alert'
        text: '{{ .CommonAnnotations.description }}'
```

### 8.2 PagerDuty Integration

```yaml
receivers:
  - name: 'storage-pagerduty'
    pagerduty_configs:
      - service_key: '<your-service-key>'
        severity: '{{ .CommonLabels.severity }}'
```

### 8.3 Custom Webhooks

For custom integrations, the operator exposes:
- Metrics endpoint for scraping
- Kubernetes events for volume migrations
- CRD status for polling

---

## 9. Troubleshooting Integrations

### 9.1 Prometheus Not Scraping

```bash
# Check ServiceMonitor
kubectl get servicemonitor -A | grep storage-operator

# Check Prometheus targets
curl http://prometheus:9090/api/v1/targets | jq '.data.activeTargets[] | select(.labels.job=="couchestor")'

# Verify service exists
kubectl get svc -n kube-system | grep storage-operator
```

### 9.2 Grafana Dashboard Not Loading

```bash
# Check data source
curl http://grafana:3000/api/datasources

# Verify Prometheus connectivity from Grafana
kubectl exec -n monitoring deploy/grafana -- wget -qO- http://prometheus:9090/-/healthy
```

### 9.3 Alerts Not Firing

```bash
# Check rule is loaded
curl http://prometheus:9090/api/v1/rules | jq '.data.groups[] | select(.name=="couchestor")'

# Check AlertManager
curl http://alertmanager:9093/api/v1/alerts
```
