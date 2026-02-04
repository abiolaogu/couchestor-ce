# Video Training Script: Troubleshooting

## Video Information

| Field | Value |
|-------|-------|
| Title | Troubleshooting Common Issues |
| Duration | 10 minutes |
| Audience | All technical users |
| Prerequisites | Videos 1-4 |

---

## Scene 1: Opening (0:00 - 0:30)

### Script

> Welcome to the final video in our series! Today we'll troubleshoot the most common issues you might encounter with the CoucheStor.
>
> I'll show you how to diagnose problems quickly and get things working again.

---

## Scene 2: Issue - Operator Not Starting (0:30 - 2:30)

### Script

> Let's start with the most critical issue: the operator won't start.
>
> First, check the pod status:
>
> ```bash
> kubectl get pods -n kube-system -l app=couchestor
> ```
>
> If it's in CrashLoopBackOff, check the logs:
>
> ```bash
> kubectl logs -n kube-system -l app=couchestor --previous
> ```
>
> Common causes:
>
> **CRD not installed:**
> You'll see "StoragePolicy CRD not found"
> Fix: `kubectl apply -f crd.yaml`
>
> **RBAC missing:**
> You'll see permission denied errors
> Fix: `kubectl apply -f rbac.yaml`
>
> **Prometheus unreachable:**
> You'll see "Prometheus connection error"
> Fix: Verify the `--prometheus-url` setting

---

## Scene 3: Issue - Policy Stuck in Pending (2:30 - 4:30)

### Script

> Next issue: your policy stays in "Pending" and never becomes "Active".
>
> ```bash
> kubectl get storagepolicy my-policy
> # Shows: PHASE=Pending
> ```
>
> Check the operator logs:
>
> ```bash
> kubectl logs -n kube-system -l app=couchestor | grep "my-policy"
> ```
>
> Common causes:
>
> **Wrong StorageClass name:**
> ```bash
> kubectl get storageclass
> # Verify your policy's storageClassName exists
> ```
>
> **No matching volumes:**
> ```bash
> kubectl get pv -o custom-columns='NAME:.metadata.name,SC:.spec.storageClassName'
> # Check if any PVs use your StorageClass
> ```
>
> The policy needs at least one matching volume to become Active.

---

## Scene 4: Issue - Migrations Not Happening (4:30 - 6:30)

### Script

> A common question: "Why isn't my volume migrating?"
>
> Let's check systematically:
>
> **Is the policy enabled?**
> ```bash
> kubectl get storagepolicy my-policy -o jsonpath='{.spec.enabled}'
> # Should be: true
> ```
>
> **Is it in dry-run mode?**
> ```bash
> kubectl get storagepolicy my-policy -o jsonpath='{.spec.dryRun}'
> # Should be: false for real migrations
> ```
>
> **What's the volume's heat score?**
> ```bash
> kubectl logs -n kube-system -l app=couchestor | grep "my-volume" | grep "heat score"
> ```
>
> If the score is between your thresholds, no migration will happen - that's expected!
>
> **Is the cooldown active?**
> ```bash
> kubectl get pv my-pv -o jsonpath='{.metadata.annotations}'
> # Check the last-migration timestamp
> ```
>
> If the cooldown hasn't elapsed, the volume won't migrate yet.

---

## Scene 5: Issue - Migrations Failing (6:30 - 8:30)

### Script

> When migrations start but fail, check the migration history:
>
> ```bash
> kubectl get storagepolicy my-policy -o json | jq '.status.migrationHistory[] | select(.success==false)'
> ```
>
> Then check the logs for details:
>
> ```bash
> kubectl logs -n kube-system -l app=couchestor | grep -A 5 "migration failed"
> ```
>
> Common failures:
>
> **"Target pool not found"**
> Your pool selector doesn't match any online pools.
> Fix: Check pool labels match your selector
> ```bash
> kubectl get diskpools --show-labels
> ```
>
> **"Sync timeout"**
> The replica took too long to sync.
> Causes: Very large volume, slow storage, Mayastor issues
> Fix: Increase `migrationTimeout` or check Mayastor health
>
> **"Target pool offline"**
> The pool is not healthy.
> Fix: Check DiskPool status with `kubectl get diskpools`

---

## Scene 6: Quick Diagnostic Commands (8:30 - 9:30)

### Script

> Here are the commands I use most for troubleshooting. Keep these handy:
>
> **Overall health:**
> ```bash
> kubectl get pods -n kube-system -l app=couchestor
> kubectl get storagepolicies
> ```
>
> **Operator logs (recent errors):**
> ```bash
> kubectl logs -n kube-system -l app=couchestor --tail=100 | grep -E "(ERROR|WARN|failed)"
> ```
>
> **Policy details:**
> ```bash
> kubectl describe storagepolicy my-policy
> ```
>
> **Pool status:**
> ```bash
> kubectl get diskpools -o wide
> ```
>
> **Test Prometheus:**
> ```bash
> kubectl exec -n kube-system -it deploy/couchestor -- wget -qO- http://prometheus:9090/-/healthy
> ```

---

## Scene 7: Closing (9:30 - 10:00)

### Script

> That wraps up our troubleshooting guide and our video series!
>
> Remember:
> - Start with health checks
> - Check logs for specific errors
> - Verify configurations match reality
> - Don't hesitate to use dry-run for testing
>
> For more complex issues, check the documentation or reach out to support.
>
> Thanks for watching the complete CoucheStor training series!
