# Deployment Guide — couchestor-ce
> Version: 1.0 | Last Updated: 2026-02-18 | Status: Draft
> Classification: Internal | Author: AIDD System

## 1. Overview

Deployment procedures for couchestor-ce across all environments.

## 2. Prerequisites

- Kubernetes cluster (v1.29+)
- Helm 3.x installed
- kubectl configured
- Container registry access
- Vault access for secrets

## 3. Environment Setup

### 3.1 Infrastructure Provisioning
```bash
# Terraform — provision infrastructure
cd infra/terraform
terraform init
terraform plan -var-file=env/production.tfvars
terraform apply
```

### 3.2 Kubernetes Namespace
```bash
kubectl create namespace couchestor-ce
kubectl label namespace couchestor-ce istio-injection=enabled
```

### 3.3 Secrets Configuration
```bash
# Create secrets from Vault
vault kv get -format=json secret/couchestor-ce/db | \
  kubectl create secret generic db-credentials --from-file=-
```

## 4. Deployment Procedure

### 4.1 Standard Deployment (Blue-Green)
```bash
# Step 1: Build and push container
docker build -t registry.billyrinks.com/couchestor-ce:$VERSION .
docker push registry.billyrinks.com/couchestor-ce:$VERSION

# Step 2: Deploy to green environment
helm upgrade --install couchestor-ce ./charts \
  --namespace couchestor-ce \
  --set image.tag=$VERSION \
  --values values/production.yaml

# Step 3: Run smoke tests
./scripts/smoke-test.sh

# Step 4: Switch traffic
kubectl patch service couchestor-ce -p '{"spec":{"selector":{"version":"green"}}}'
```

### 4.2 Database Migrations
```bash
# Run migrations before deployment
migrate -path ./migrations -database "$DATABASE_URL" up
```

## 5. Rollback Procedure

```bash
# Immediate rollback — switch back to blue
kubectl patch service couchestor-ce -p '{"spec":{"selector":{"version":"blue"}}}'

# Or rollback Helm release
helm rollback couchestor-ce --namespace couchestor-ce
```

## 6. Health Verification

```bash
# Check pod status
kubectl get pods -n couchestor-ce

# Check service health
curl https://api.billyrinks.com/v1/health

# Verify metrics
curl http://prometheus:9090/api/v1/query?query=up{job="couchestor-ce"}
```

## 7. Monitoring Post-Deploy

| Check | Threshold | Action |
|-------|-----------|--------|
| Error rate | > 1% | Rollback |
| Latency (p99) | > 1s | Investigate |
| CPU usage | > 80% | Scale up |
| Memory usage | > 85% | Scale up |
| Pod restarts | > 3 in 5min | Rollback |

## 8. Deployment Schedule

| Environment | Schedule | Approval |
|------------|----------|----------|
| Development | On commit | Automatic |
| Staging | Daily | Automatic |
| Production | Weekly (Tue 10am) | Manual |
| Hotfix | As needed | CTO approval |
