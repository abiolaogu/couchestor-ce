# Video Training Script: Configuration

## Video Information

| Field | Value |
|-------|-------|
| Title | Configuring Storage Policies |
| Duration | 12 minutes |
| Audience | Platform Engineers, Developers |
| Prerequisites | Video 2: Installation |

---

## Scene 1: Opening (0:00 - 0:30)

### Script

> Welcome back! In this video, we'll configure our first StoragePolicy and see the CoucheStor in action.
>
> By the end of this video, you'll know how to create policies, set appropriate thresholds, and use label selectors to target specific workloads.

---

## Scene 2: Preparing DiskPools (0:30 - 2:30)

### Visuals
- Terminal showing kubectl commands

### Script

> Before we create a policy, we need to label our DiskPools so the operator knows which pools are for hot storage and which are for cold storage.
>
> [Type command]
>
> ```bash
> kubectl get diskpools
> ```
>
> We can see our pools. Let's label them:
>
> ```bash
> kubectl label diskpool pool-nvme-1 tier=hot media=nvme
> kubectl label diskpool pool-sata-1 tier=cold media=sata
> ```
>
> Now verify:
>
> ```bash
> kubectl get diskpools --show-labels
> ```
>
> Great! Our pools are labeled and ready.

---

## Scene 3: Creating a Basic Policy (2:30 - 5:30)

### Visuals
- YAML editor and terminal

### Script

> Let's create our first StoragePolicy. I'll walk you through each field.
>
> [Show YAML building up line by line]
>
> ```yaml
> apiVersion: storage.billyronks.io/v1
> kind: StoragePolicy
> metadata:
>   name: my-first-policy
> spec:
>   storageClassName: mayastor
>   highWatermarkIOPS: 5000
>   lowWatermarkIOPS: 500
>   samplingWindow: "1h"
>   cooldownPeriod: "24h"
>   nvmePoolSelector:
>     matchLabels:
>       tier: hot
>   sataPoolSelector:
>     matchLabels:
>       tier: cold
>   enabled: true
>   dryRun: true
> ```
>
> Let me explain each part:
> - `storageClassName`: Which volumes to manage
> - `highWatermarkIOPS`: Move to NVMe if above 5000 IOPS
> - `lowWatermarkIOPS`: Move to SATA if below 500 IOPS
> - `samplingWindow`: Average IOPS over 1 hour
> - `cooldownPeriod`: Wait 24 hours between migrations
> - Pool selectors: Match our labeled pools
> - `dryRun: true`: Safe mode - log only, don't migrate
>
> Let's apply it:
>
> ```bash
> kubectl apply -f policy.yaml
> ```

---

## Scene 4: Verifying the Policy (5:30 - 7:30)

### Visuals
- Terminal showing status checks

### Script

> Let's check our policy:
>
> ```bash
> kubectl get storagepolicies
> ```
>
> We see it's "Active" and watching some volumes. Let's get more details:
>
> ```bash
> kubectl describe storagepolicy my-first-policy
> ```
>
> This shows the full status including how many volumes it's managing.
>
> Now let's check the operator logs to see what it's doing:
>
> ```bash
> kubectl logs -n kube-system -l app=couchestor --tail=30
> ```
>
> See these lines? The operator is evaluating each volume and making decisions. Since we're in dry-run mode, it's logging what it would do without actually migrating anything.

---

## Scene 5: Advanced Configuration (7:30 - 10:00)

### Visuals
- YAML examples

### Script

> Now let's look at some advanced configuration options.
>
> **Targeting specific volumes with labels:**
>
> ```yaml
> spec:
>   volumeSelector:
>     matchLabels:
>       app: postgresql
>       environment: production
> ```
>
> This policy will only manage volumes with both labels.
>
> **Using match expressions for complex selections:**
>
> ```yaml
> spec:
>   nvmePoolSelector:
>     matchExpressions:
>       - key: region
>         operator: In
>         values: [us-east, us-west]
> ```
>
> This selects pools in either US region.
>
> **Adjusting concurrency and timeouts:**
>
> ```yaml
> spec:
>   maxConcurrentMigrations: 4
>   migrationTimeout: "45m"
> ```
>
> For large deployments or large volumes.

---

## Scene 6: Enabling Live Migrations (10:00 - 11:30)

### Visuals
- Terminal showing the transition from dry-run to live

### Script

> Once you're confident your policy is configured correctly, it's time to enable live migrations.
>
> First, review the dry-run logs one more time:
>
> ```bash
> kubectl logs -n kube-system -l app=couchestor | grep "DRY-RUN"
> ```
>
> If everything looks good, disable dry-run:
>
> ```bash
> kubectl patch storagepolicy my-first-policy --type=merge -p '{"spec":{"dryRun":false}}'
> ```
>
> Now the operator will actually migrate volumes when thresholds are crossed.
>
> Watch the logs:
>
> ```bash
> kubectl logs -n kube-system -l app=couchestor -f
> ```
>
> You'll see real migrations happening!

---

## Scene 7: Closing (11:30 - 12:00)

### Visuals
- Summary and next video preview

### Script

> That's it for configuration! You've learned:
> - How to label DiskPools
> - How to create a StoragePolicy
> - How to use selectors for targeting
> - How to safely test with dry-run mode
>
> In the next video, we'll cover day-to-day operations and monitoring.
>
> Thanks for watching!
