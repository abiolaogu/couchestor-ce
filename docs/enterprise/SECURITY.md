# Security Architecture Document

## Document Information

| Field | Value |
|-------|-------|
| Version | 1.0.0 |
| Status | Approved |
| Author | BillyRonks Security Team |
| Last Updated | 2026-02-02 |

---

## 1. Security Overview

The CoucheStor is designed with security as a foundational principle, implementing defense-in-depth strategies appropriate for enterprise Kubernetes environments.

---

## 2. Threat Model

### 2.1 Trust Boundaries

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                          Trust Boundary Diagram                              │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  ┌─ Cluster Boundary ─────────────────────────────────────────────────────┐ │
│  │                                                                         │ │
│  │  ┌─ Operator Namespace ───────────────────────────────────────────┐    │ │
│  │  │                                                                 │    │ │
│  │  │  ┌─────────────────────────────────────────────────────────┐   │    │ │
│  │  │  │              CoucheStor                      │   │    │ │
│  │  │  │              (Trust Zone: Internal)                      │   │    │ │
│  │  │  └─────────────────────────────────────────────────────────┘   │    │ │
│  │  │                                                                 │    │ │
│  │  └─────────────────────────────────────────────────────────────────┘    │ │
│  │                            │                                            │ │
│  │           ┌────────────────┼────────────────┐                          │ │
│  │           │                │                │                          │ │
│  │           ▼                ▼                ▼                          │ │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐                    │ │
│  │  │ K8s API     │  │ Prometheus  │  │ Mayastor    │                    │ │
│  │  │ (Trusted)   │  │ (Semi-trust)│  │ (Trusted)   │                    │ │
│  │  └─────────────┘  └─────────────┘  └─────────────┘                    │ │
│  │                                                                         │ │
│  └─────────────────────────────────────────────────────────────────────────┘ │
│                                                                              │
│  External (Untrusted):                                                      │
│  • No direct external access                                                │
│  • All access via K8s API (authenticated)                                   │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 2.2 Attack Surface

| Attack Vector | Risk Level | Mitigation |
|--------------|------------|------------|
| Malicious CRD | Medium | RBAC, validation |
| Prometheus injection | Low | Query sanitization |
| Container escape | Low | Non-root, read-only FS |
| Network sniffing | Low | Internal traffic only |
| Denial of service | Medium | Resource limits |

### 2.3 STRIDE Analysis

| Threat | Applicable | Mitigation |
|--------|------------|------------|
| **S**poofing | Low | ServiceAccount authentication |
| **T**ampering | Medium | RBAC, audit logging |
| **R**epudiation | Low | Audit logs |
| **I**nformation Disclosure | Low | No secrets handled |
| **D**enial of Service | Medium | Resource limits, rate limiting |
| **E**levation of Privilege | Low | Least-privilege RBAC |

---

## 3. Authentication & Authorization

### 3.1 Kubernetes Authentication

```yaml
# ServiceAccount for operator
apiVersion: v1
kind: ServiceAccount
metadata:
  name: couchestor
  namespace: kube-system
automountServiceAccountToken: true
```

### 3.2 RBAC Configuration

```yaml
# ClusterRole with least-privilege permissions
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRole
metadata:
  name: couchestor
rules:
  # StoragePolicy CRD - full access (operator's own resources)
  - apiGroups: ["storage.billyronks.io"]
    resources: ["storagepolicies"]
    verbs: ["get", "list", "watch"]
  - apiGroups: ["storage.billyronks.io"]
    resources: ["storagepolicies/status"]
    verbs: ["get", "update", "patch"]

  # Mayastor resources - read and modify
  - apiGroups: ["openebs.io"]
    resources: ["diskpools"]
    verbs: ["get", "list", "watch"]
  - apiGroups: ["openebs.io"]
    resources: ["mayastorvolumes"]
    verbs: ["get", "list", "watch", "update", "patch"]

  # PersistentVolumes - read and annotate
  - apiGroups: [""]
    resources: ["persistentvolumes"]
    verbs: ["get", "list", "watch", "update", "patch"]

  # Events - create for audit trail
  - apiGroups: [""]
    resources: ["events"]
    verbs: ["create", "patch"]

  # Coordination for leader election (optional)
  - apiGroups: ["coordination.k8s.io"]
    resources: ["leases"]
    verbs: ["get", "create", "update"]
```

### 3.3 Permission Justification

| Permission | Resource | Justification |
|------------|----------|---------------|
| get, list, watch | StoragePolicy | Monitor policy changes |
| update, patch | StoragePolicy/status | Report current state |
| get, list, watch | DiskPool | Find target pools |
| update, patch | MayastorVolume | Execute migrations |
| update, patch | PersistentVolume | Record annotations |
| create | Events | Audit trail |

---

## 4. Network Security

### 4.1 Network Policy

```yaml
# Restrict operator network access
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: couchestor
  namespace: kube-system
spec:
  podSelector:
    matchLabels:
      app: couchestor
  policyTypes:
    - Ingress
    - Egress
  ingress:
    # Allow Prometheus scraping
    - from:
        - namespaceSelector:
            matchLabels:
              name: monitoring
      ports:
        - port: 8080
          protocol: TCP
  egress:
    # Allow Kubernetes API
    - to:
        - ipBlock:
            cidr: 0.0.0.0/0
      ports:
        - port: 443
          protocol: TCP
        - port: 6443
          protocol: TCP
    # Allow Prometheus queries
    - to:
        - namespaceSelector:
            matchLabels:
              name: monitoring
      ports:
        - port: 9090
          protocol: TCP
    # Allow DNS
    - to:
        - namespaceSelector: {}
      ports:
        - port: 53
          protocol: UDP
        - port: 53
          protocol: TCP
```

### 4.2 Traffic Flow

| Direction | Source | Destination | Port | Purpose |
|-----------|--------|-------------|------|---------|
| Egress | Operator | K8s API | 443/6443 | API calls |
| Egress | Operator | Prometheus | 9090 | Metrics query |
| Ingress | Prometheus | Operator | 8080 | Metrics scrape |
| Ingress | K8s | Operator | 8081 | Health probes |

---

## 5. Container Security

### 5.1 Pod Security Context

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: couchestor
spec:
  template:
    spec:
      securityContext:
        runAsNonRoot: true
        runAsUser: 65534
        runAsGroup: 65534
        fsGroup: 65534
        seccompProfile:
          type: RuntimeDefault
      containers:
        - name: operator
          securityContext:
            allowPrivilegeEscalation: false
            readOnlyRootFilesystem: true
            capabilities:
              drop:
                - ALL
          resources:
            requests:
              cpu: 100m
              memory: 128Mi
            limits:
              cpu: 500m
              memory: 256Mi
```

### 5.2 Security Hardening Checklist

| Control | Status | Notes |
|---------|--------|-------|
| Non-root user | Implemented | UID 65534 (nobody) |
| Read-only filesystem | Implemented | No writable paths needed |
| Dropped capabilities | Implemented | ALL capabilities dropped |
| No privilege escalation | Implemented | Explicitly disabled |
| Seccomp profile | Implemented | RuntimeDefault |
| Resource limits | Implemented | CPU and memory |
| No host access | Implemented | No hostNetwork/PID/IPC |

---

## 6. Data Security

### 6.1 Data Classification

| Data Type | Classification | Handling |
|-----------|---------------|----------|
| Volume names | Internal | Logged, not encrypted |
| IOPS metrics | Internal | Cached in memory |
| Pool labels | Internal | Read from K8s API |
| Policy config | Internal | Stored in etcd |
| Migration history | Internal | In CRD status |

### 6.2 Sensitive Data Handling

**No sensitive data is processed by the operator:**
- No credentials stored or managed
- No encryption keys handled
- No PII or customer data accessed
- Volume contents are never read

### 6.3 Logging Security

```rust
// Safe logging - no sensitive data
info!("Migration completed: {} from {} to {}", volume_name, source, target);

// Avoid logging:
// - Full API responses (may contain secrets)
// - Authentication tokens
// - Volume contents
```

---

## 7. Supply Chain Security

### 7.1 Dependency Management

| Dependency | Version Pinning | Audit Status |
|------------|-----------------|--------------|
| kube-rs | Pinned (0.88) | Trusted |
| tokio | Pinned (1.36) | Trusted |
| reqwest | Pinned (0.11) | Trusted |
| serde | Pinned (1.0) | Trusted |

### 7.2 Container Image Security

```dockerfile
# Build stage
FROM rust:1.75-slim AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

# Runtime stage
FROM gcr.io/distroless/cc-debian12
COPY --from=builder /app/target/release/couchestor /
USER 65534:65534
ENTRYPOINT ["/couchestor"]
```

**Image Security Measures:**
- Distroless base image (minimal attack surface)
- Non-root user
- No shell or package manager
- Regularly updated base image

### 7.3 SBOM (Software Bill of Materials)

Generate with:
```bash
cargo sbom > sbom.json
```

---

## 8. Vulnerability Management

### 8.1 Security Scanning

| Tool | Purpose | Frequency |
|------|---------|-----------|
| cargo audit | Dependency vulnerabilities | CI/CD |
| trivy | Container image scanning | CI/CD |
| kube-bench | K8s security | Monthly |

### 8.2 Vulnerability Response

| Severity | Response Time | Action |
|----------|--------------|--------|
| Critical | 24 hours | Patch release |
| High | 7 days | Patch release |
| Medium | 30 days | Next release |
| Low | 90 days | Next release |

---

## 9. Audit & Compliance

### 9.1 Audit Events

All significant operations are logged:

```json
{
  "timestamp": "2026-02-02T10:30:00Z",
  "event": "migration_started",
  "volume": "pvc-abc123",
  "source_pool": "sata-pool-1",
  "target_pool": "nvme-pool-1",
  "policy": "database-tiering",
  "trigger": "iops_threshold",
  "trigger_value": 6500.0
}
```

### 9.2 Compliance Controls

| Control | Implementation | Evidence |
|---------|---------------|----------|
| Access control | RBAC | ClusterRole definition |
| Audit logging | Structured logs | Log aggregation |
| Change management | GitOps | Git history |
| Least privilege | Scoped permissions | RBAC review |
| Encryption in transit | TLS (K8s API) | Default |

---

## 10. Incident Response

### 10.1 Security Incident Playbook

**Detection:**
1. Monitor operator logs for anomalies
2. Alert on unexpected API calls
3. Monitor resource usage spikes

**Response:**
1. Isolate: Set all policies `enabled: false`
2. Investigate: Review logs and events
3. Contain: Delete suspicious policies
4. Recover: Restore from GitOps
5. Report: Document incident

### 10.2 Emergency Procedures

```bash
# Disable all tiering immediately
kubectl patch storagepolicy --all --type=merge -p '{"spec":{"enabled":false}}'

# Stop operator
kubectl scale deployment couchestor --replicas=0 -n kube-system

# Review recent migrations
kubectl get storagepolicies -o yaml | grep -A 50 migrationHistory
```
