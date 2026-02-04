# Product Requirements Document (PRD)

## Document Information

| Field | Value |
|-------|-------|
| Product Name | CoucheStor |
| Version | 1.0.0 |
| Status | Approved |
| Author | BillyRonks Product Team |
| Last Updated | 2026-02-02 |

---

## 1. Executive Summary

### 1.1 Product Vision

The CoucheStor provides intelligent, automated storage tiering for Kubernetes environments running OpenEBS Mayastor. It eliminates the operational burden of manually managing storage placement by automatically migrating workloads between high-performance NVMe and cost-effective SATA storage tiers based on actual usage patterns.

### 1.2 Problem Statement

Organizations using Mayastor face these challenges:

1. **Inefficient Resource Utilization**: High-performance NVMe storage is expensive, but organizations over-provision it for workloads that don't require high IOPS, leading to wasted resources.

2. **Manual Tiering Operations**: Without automation, storage administrators must manually identify candidates for migration and execute complex data movement operations.

3. **Performance Degradation Risk**: Workloads that suddenly require high performance may be stuck on slower storage, impacting application performance.

4. **Operational Complexity**: Managing storage placement at scale requires continuous monitoring and expert knowledge.

### 1.3 Solution Overview

The CoucheStor solves these problems by:

- **Continuous Monitoring**: Automatically collecting IOPS metrics from Prometheus
- **Intelligent Decision Making**: Using configurable policies to determine optimal storage placement
- **Safe Automated Migration**: Executing data migrations with built-in safety guarantees
- **Policy-Driven Configuration**: Allowing administrators to define tiering rules declaratively

---

## 2. Goals and Objectives

### 2.1 Business Goals

| Goal | Success Metric | Target |
|------|---------------|--------|
| Reduce storage costs | Cost per GB stored | 30% reduction |
| Improve operational efficiency | Manual interventions per week | 90% reduction |
| Maintain performance SLAs | P99 latency for hot workloads | < 1ms |
| Reduce time-to-value | Time from deployment to active tiering | < 1 hour |

### 2.2 Technical Objectives

| Objective | Requirement |
|-----------|-------------|
| Data Safety | Zero data loss during migrations |
| Availability | No application downtime during migrations |
| Scalability | Support 1000+ volumes per cluster |
| Reliability | 99.9% operator uptime |
| Observability | Complete visibility into all operations |

---

## 3. User Personas

### 3.1 Platform Engineer (Primary User)

**Profile**:
- Responsible for Kubernetes infrastructure
- Manages storage subsystems including Mayastor
- Defines organizational policies for storage usage

**Needs**:
- Simple declarative configuration
- Integration with existing monitoring (Prometheus)
- Clear visibility into system behavior
- Confidence that data is safe

**Pain Points**:
- Manual storage management is time-consuming
- Difficult to track which volumes need migration
- Risk of data loss during manual operations

### 3.2 Application Developer (Secondary User)

**Profile**:
- Deploys applications to Kubernetes
- Consumes storage via PersistentVolumeClaims
- Focused on application functionality, not storage details

**Needs**:
- Transparent operation (no changes to application deployment)
- Consistent storage performance
- No manual intervention required

**Pain Points**:
- Application performance varies unpredictably
- No insight into why storage behaves differently over time

### 3.3 Finance/Operations Manager (Stakeholder)

**Profile**:
- Responsible for infrastructure costs
- Tracks resource utilization and efficiency

**Needs**:
- Cost reduction through optimized resource usage
- Reports on storage tier distribution
- Predictable budgeting

**Pain Points**:
- Over-provisioning leads to wasted budget
- Difficult to justify NVMe costs without utilization data

---

## 4. Feature Requirements

### 4.1 Core Features

#### 4.1.1 Policy-Based Tiering (P0 - Must Have)

**Description**: Users can define StoragePolicy custom resources that specify tiering rules.

**Requirements**:
| ID | Requirement | Priority |
|----|-------------|----------|
| POL-001 | Support CRD-based policy definition | P0 |
| POL-002 | Configure IOPS thresholds (high/low watermarks) | P0 |
| POL-003 | Configure time windows for averaging | P0 |
| POL-004 | Configure cooldown periods between migrations | P0 |
| POL-005 | Select target pools by labels | P0 |
| POL-006 | Filter volumes by StorageClass | P0 |
| POL-007 | Enable/disable policies dynamically | P0 |

**Acceptance Criteria**:
- [ ] User can create StoragePolicy via kubectl apply
- [ ] Policy validates input parameters
- [ ] Policy status shows current state
- [ ] Changes to policy take effect within 5 minutes

#### 4.1.2 Metrics Collection (P0 - Must Have)

**Description**: Operator collects volume performance metrics from Prometheus.

**Requirements**:
| ID | Requirement | Priority |
|----|-------------|----------|
| MET-001 | Query Prometheus for volume IOPS | P0 |
| MET-002 | Support configurable metric names | P0 |
| MET-003 | Support fallback metrics | P0 |
| MET-004 | Calculate time-weighted averages | P0 |
| MET-005 | Cache results to reduce load | P1 |
| MET-006 | Handle Prometheus unavailability gracefully | P0 |

**Acceptance Criteria**:
- [ ] Operator queries Prometheus successfully
- [ ] Heat scores are calculated correctly
- [ ] Prometheus downtime doesn't crash operator
- [ ] Cache reduces query frequency by 50%+

#### 4.1.3 Safe Volume Migration (P0 - Must Have)

**Description**: Operator executes volume migrations with data safety guarantees.

**Requirements**:
| ID | Requirement | Priority |
|----|-------------|----------|
| MIG-001 | Never remove old replica before new is synced | P0 |
| MIG-002 | Timeout stuck migrations | P0 |
| MIG-003 | Support dry-run mode | P0 |
| MIG-004 | Support preservation mode | P1 |
| MIG-005 | Limit concurrent migrations | P0 |
| MIG-006 | Track migration progress | P0 |
| MIG-007 | Record migration history | P1 |

**Acceptance Criteria**:
- [ ] No data loss in any failure scenario
- [ ] Migrations timeout after configured duration
- [ ] Dry-run logs decisions without making changes
- [ ] Concurrent migration limit is enforced

### 4.2 Operational Features

#### 4.2.1 Observability (P0 - Must Have)

**Requirements**:
| ID | Requirement | Priority |
|----|-------------|----------|
| OBS-001 | Expose Prometheus metrics | P0 |
| OBS-002 | Structured logging (JSON) | P0 |
| OBS-003 | Health check endpoints | P0 |
| OBS-004 | Policy status reporting | P0 |

#### 4.2.2 Configuration (P1 - Should Have)

**Requirements**:
| ID | Requirement | Priority |
|----|-------------|----------|
| CFG-001 | Environment variable configuration | P0 |
| CFG-002 | Command-line arguments | P0 |
| CFG-003 | Sensible defaults for all options | P0 |

### 4.3 Non-Functional Requirements

#### 4.3.1 Performance

| Requirement | Target |
|-------------|--------|
| Reconciliation time | < 30 seconds per policy |
| Prometheus query latency | < 5 seconds |
| Memory usage | < 256 MB |
| CPU usage (idle) | < 50m |

#### 4.3.2 Reliability

| Requirement | Target |
|-------------|--------|
| Operator availability | 99.9% |
| Error recovery time | < 60 seconds |
| Data loss tolerance | Zero |

#### 4.3.3 Security

| Requirement | Description |
|-------------|-------------|
| RBAC | Least privilege access to K8s API |
| Network | No ingress required |
| Secrets | No sensitive data in logs |

---

## 5. User Stories

### 5.1 Policy Management

**US-001**: As a Platform Engineer, I want to define storage tiering policies declaratively, so that I can manage them with GitOps workflows.

**US-002**: As a Platform Engineer, I want to enable dry-run mode for new policies, so that I can validate behavior before enabling automatic migrations.

**US-003**: As a Platform Engineer, I want to set different thresholds for different workload types, so that I can optimize storage for each use case.

### 5.2 Migration Operations

**US-004**: As a Platform Engineer, I want migrations to be safe by default, so that I don't have to worry about data loss.

**US-005**: As a Platform Engineer, I want to see the history of migrations, so that I can audit what changes were made.

**US-006**: As a Platform Engineer, I want to limit concurrent migrations, so that I don't overwhelm the storage system.

### 5.3 Monitoring

**US-007**: As a Platform Engineer, I want to see operator metrics in Prometheus, so that I can create dashboards and alerts.

**US-008**: As a Platform Engineer, I want to see the current state of each policy, so that I know the system is working correctly.

### 5.4 Operations

**US-009**: As a Platform Engineer, I want the operator to restart automatically after failures, so that I don't have to manually intervene.

**US-010**: As a Platform Engineer, I want clear error messages in logs, so that I can troubleshoot issues quickly.

---

## 6. Constraints and Assumptions

### 6.1 Constraints

| Constraint | Impact |
|------------|--------|
| Requires OpenEBS Mayastor | Not compatible with other storage providers |
| Requires Prometheus | Metrics source is not pluggable |
| Kubernetes 1.25+ | Uses features from recent K8s versions |
| Single cluster only | No multi-cluster support in v1.0 |

### 6.2 Assumptions

| Assumption | Risk if False |
|------------|---------------|
| Prometheus has volume IOPS metrics | Operator cannot make tiering decisions |
| Mayastor volumes support topology changes | Migrations will fail |
| Network latency < 100ms to Prometheus | Query timeouts may occur |
| DiskPools have appropriate labels | Pool selection will fail |

---

## 7. Success Metrics

### 7.1 Adoption Metrics

| Metric | Target (6 months) |
|--------|------------------|
| Organizations using operator | 50+ |
| Volumes managed | 10,000+ |
| GitHub stars | 500+ |

### 7.2 Operational Metrics

| Metric | Target |
|--------|--------|
| Migration success rate | > 99% |
| Mean time between failures | > 720 hours |
| Time to resolve issues | < 4 hours |

### 7.3 Business Impact Metrics

| Metric | Target |
|--------|--------|
| Storage cost reduction | 30% |
| Admin time saved | 10 hours/week |
| Performance SLA compliance | 99.9% |

---

## 8. Release Plan

### 8.1 Version 1.0.0 (Current)

**Scope**: Core tiering functionality
- StoragePolicy CRD
- Prometheus metrics collection
- Safe volume migration
- Basic observability

### 8.2 Version 1.1.0 (Future)

**Scope**: Enhanced observability
- Grafana dashboard templates
- Alert rule examples
- Detailed migration events

### 8.3 Version 1.2.0 (Future)

**Scope**: Advanced features
- Predictive tiering based on time-of-day patterns
- Cost-based optimization
- Multi-cluster support

---

## 9. Appendix

### 9.1 Glossary

| Term | Definition |
|------|------------|
| Hot Tier | High-performance storage (NVMe) |
| Cold Tier | Cost-effective storage (SATA/HDD) |
| Heat Score | Numeric representation of volume activity |
| Watermark | IOPS threshold that triggers migration |
| Cooldown | Minimum time between migrations |

### 9.2 References

- OpenEBS Mayastor Documentation
- Kubernetes Operator Pattern
- Prometheus Query Language (PromQL)
