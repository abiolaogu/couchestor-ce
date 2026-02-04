# Video Training Script: Operations

## Video Information

| Field | Value |
|-------|-------|
| Title | Day-to-Day Operations |
| Duration | 10 minutes |
| Audience | Operations Teams, SREs |
| Prerequisites | Video 3: Configuration |

---

## Scene 1: Opening (0:00 - 0:30)

### Script

> Welcome back! In this video, we'll cover the day-to-day operations of the CoucheStor - how to monitor it, perform common tasks, and keep it running smoothly.

---

## Scene 2: Health Monitoring (0:30 - 3:00)

### Script

> Let's start with health monitoring. The first thing to check is the operator itself:
>
> ```bash
> kubectl get pods -n kube-system -l app=couchestor
> ```
>
> You want to see "Running" with "1/1" Ready.
>
> Next, check your policies:
>
> ```bash
> kubectl get storagepolicies
> ```
>
> All policies should show "Active" in the PHASE column.
>
> For more detail on a specific policy:
>
> ```bash
> kubectl get storagepolicy my-policy -o yaml | grep -A 20 "^status:"
> ```
>
> This shows watched volumes, migration counts, and recent activity.
>
> Don't forget to check the logs for any warnings:
>
> ```bash
> kubectl logs -n kube-system -l app=couchestor --tail=50 | grep -E "(WARN|ERROR)"
> ```

---

## Scene 3: Common Operations (3:00 - 6:00)

### Script

> Let's go through common operational tasks.
>
> **Pausing a policy:**
>
> Sometimes you need to stop migrations, like during maintenance:
>
> ```bash
> kubectl patch storagepolicy my-policy --type=merge -p '{"spec":{"enabled":false}}'
> ```
>
> **Resuming a policy:**
>
> ```bash
> kubectl patch storagepolicy my-policy --type=merge -p '{"spec":{"enabled":true}}'
> ```
>
> **Adjusting thresholds:**
>
> If you're seeing too many or too few migrations:
>
> ```bash
> kubectl patch storagepolicy my-policy --type=merge -p '{"spec":{"highWatermarkIOPS":3000}}'
> ```
>
> **Viewing migration history:**
>
> ```bash
> kubectl get storagepolicy my-policy -o jsonpath='{.status.migrationHistory}' | jq '.[0:5]'
> ```
>
> This shows the last 5 migrations with details like duration and trigger IOPS.

---

## Scene 4: Monitoring with Grafana (6:00 - 8:00)

### Visuals
- Grafana dashboard screenshots

### Script

> For ongoing monitoring, you'll want a Grafana dashboard. The operator exposes Prometheus metrics on port 8080.
>
> Key metrics to watch:
>
> - `storage_operator_reconcile_total` - Are reconciliations happening?
> - `storage_operator_migrations_total{status="success"}` - Successful migrations
> - `storage_operator_migrations_total{status="failed"}` - Failed migrations
> - `storage_operator_active_migrations` - Currently running
>
> Set up alerts for:
> - Operator down for more than 5 minutes
> - Failed migration rate above 5%
> - Policy stuck in Error state
>
> These alerts will catch problems early.

---

## Scene 5: Maintenance Tasks (8:00 - 9:30)

### Script

> For planned maintenance, follow these steps:
>
> **Before maintenance:**
>
> ```bash
> # Disable all policies
> kubectl patch storagepolicy --all --type=merge -p '{"spec":{"enabled":false}}'
>
> # Wait for active migrations to complete
> watch kubectl get storagepolicies -o custom-columns='NAME:.metadata.name,ACTIVE:.status.activeMigrations'
> ```
>
> Wait until all show 0 active migrations.
>
> **After maintenance:**
>
> ```bash
> # Verify operator is healthy
> kubectl get pods -n kube-system -l app=couchestor
>
> # Re-enable policies
> kubectl patch storagepolicy --all --type=merge -p '{"spec":{"enabled":true}}'
> ```
>
> Always verify the operator reconnects to Prometheus by checking the logs.

---

## Scene 6: Closing (9:30 - 10:00)

### Script

> To summarize today's operations topics:
> - Monitor operator and policy health regularly
> - Use simple patches for common tasks
> - Set up Grafana dashboards and alerts
> - Follow proper maintenance procedures
>
> In the final video, we'll cover troubleshooting common issues.
>
> See you there!
