# Video Training Script: Installation

## Video Information

| Field | Value |
|-------|-------|
| Title | Installing the CoucheStor |
| Duration | 10 minutes |
| Audience | Platform Engineers, Administrators |
| Prerequisites | Video 1: Introduction |

---

## Scene 1: Opening (0:00 - 0:30)

### Visuals
- Title card: "Installing the CoucheStor"
- Terminal window ready

### Script

> Welcome back! In this video, we'll install the CoucheStor in a Kubernetes cluster. I'll walk you through each step, and by the end, you'll have a working operator ready to configure.
>
> Before we begin, make sure you have kubectl configured and cluster admin access. You should also have Mayastor and Prometheus already installed.

---

## Scene 2: Prerequisites Check (0:30 - 2:00)

### Visuals
- Terminal showing commands
- Checkmark icons for each prerequisite

### Script

> Let's verify our prerequisites first.
>
> [Type command]
>
> ```bash
> kubectl version --short
> ```
>
> We need Kubernetes 1.25 or later. Good, we have 1.28.
>
> [Type command]
>
> ```bash
> kubectl get pods -n mayastor
> ```
>
> This shows Mayastor is installed and running.
>
> [Type command]
>
> ```bash
> kubectl get pods -n monitoring | grep prometheus
> ```
>
> And Prometheus is up. Now let's check that we have the right permissions.
>
> [Type command]
>
> ```bash
> kubectl auth can-i create customresourcedefinitions
> ```
>
> Yes - we have cluster admin access. We're ready to install.

---

## Scene 3: Install CRD (2:00 - 3:30)

### Visuals
- Terminal showing CRD installation
- Diagram explaining CRDs

### Script

> The first step is installing the Custom Resource Definition. The CRD tells Kubernetes about our new resource type - StoragePolicy.
>
> [Type command]
>
> ```bash
> kubectl apply -f https://raw.githubusercontent.com/billyronks/couchestor/main/manifests/crd.yaml
> ```
>
> [Show output]
>
> Great, the CRD is created. Let's verify it:
>
> [Type command]
>
> ```bash
> kubectl get crd storagepolicies.storage.billyronks.io
> ```
>
> Perfect. Now Kubernetes knows about StoragePolicy resources. You can even see the schema with `kubectl explain`:
>
> [Type command]
>
> ```bash
> kubectl explain storagepolicy.spec
> ```
>
> This shows all the fields you can configure.

---

## Scene 4: Install RBAC (3:30 - 5:00)

### Visuals
- Terminal showing RBAC installation
- Diagram of RBAC components

### Script

> Next, we set up the security permissions. This creates a ServiceAccount for the operator and grants it the permissions it needs.
>
> [Type command]
>
> ```bash
> kubectl apply -f https://raw.githubusercontent.com/billyronks/couchestor/main/manifests/rbac.yaml
> ```
>
> [Show output]
>
> This creates three things:
> - A ServiceAccount named `couchestor`
> - A ClusterRole defining the permissions
> - A ClusterRoleBinding connecting them
>
> Let's verify:
>
> [Type commands]
>
> ```bash
> kubectl get serviceaccount couchestor -n kube-system
> kubectl get clusterrole couchestor
> ```
>
> The operator follows the principle of least privilege - it only has permissions for the specific resources it needs to manage.

---

## Scene 5: Install Deployment (5:00 - 7:00)

### Visuals
- Terminal showing deployment
- Pod status progression

### Script

> Now we deploy the operator itself:
>
> [Type command]
>
> ```bash
> kubectl apply -f https://raw.githubusercontent.com/billyronks/couchestor/main/manifests/deployment.yaml
> ```
>
> Let's watch it come up:
>
> [Type command]
>
> ```bash
> kubectl get pods -n kube-system -l app=couchestor -w
> ```
>
> [Wait for Running state]
>
> Excellent - the pod is running! Let's check the logs to make sure everything started correctly:
>
> [Type command]
>
> ```bash
> kubectl logs -n kube-system -l app=couchestor
> ```
>
> [Highlight key log lines]
>
> See these lines? "Starting CoucheStor" and "Prometheus connection healthy" tell us the operator is running and can reach Prometheus.

---

## Scene 6: Verify Installation (7:00 - 8:30)

### Visuals
- Terminal showing health checks
- Checklist graphic

### Script

> Let's do a complete verification. First, check the health endpoint:
>
> [Type command]
>
> ```bash
> kubectl port-forward -n kube-system svc/couchestor 8081:8081 &
> curl localhost:8081/healthz
> ```
>
> We get "ok" - the operator is healthy.
>
> Let's also check the metrics endpoint:
>
> [Type command]
>
> ```bash
> kubectl port-forward -n kube-system svc/couchestor 8080:8080 &
> curl localhost:8080/metrics | head -20
> ```
>
> Great - metrics are being exposed. Prometheus can scrape these.
>
> Finally, let's verify the CRD is working by creating a test policy:
>
> [Type command]
>
> ```bash
> kubectl apply -f - <<EOF
> apiVersion: storage.billyronks.io/v1
> kind: StoragePolicy
> metadata:
>   name: test-policy
> spec:
>   enabled: false
> EOF
> ```
>
> [Type command]
>
> ```bash
> kubectl get storagepolicies
> ```
>
> The policy was created successfully. Our installation is complete!

---

## Scene 7: Configuration Options (8:30 - 9:30)

### Visuals
- Table of configuration options
- Deployment YAML highlighted

### Script

> Before we wrap up, let's talk about configuration. The operator can be configured via environment variables or command-line arguments.
>
> [Show configuration table]
>
> The most important ones are:
> - `PROMETHEUS_URL` - if your Prometheus is at a different address
> - `MAX_CONCURRENT_MIGRATIONS` - limit parallel migrations
> - `DRY_RUN` - enable globally to test without making changes
>
> To modify these, edit the deployment:
>
> [Type command]
>
> ```bash
> kubectl edit deployment couchestor -n kube-system
> ```
>
> Or use `kubectl set env` for quick changes.

---

## Scene 8: Closing (9:30 - 10:00)

### Visuals
- Summary checklist
- Preview of next video

### Script

> That's it! Let's recap what we did:
>
> [Checklist appears]
>
> - Verified prerequisites
> - Installed the CRD
> - Set up RBAC permissions
> - Deployed the operator
> - Verified everything is working
>
> In the next video, we'll configure our first StoragePolicy and see the operator in action.
>
> See you there!

---

## Post-Production Notes

### Terminal Setup
- Use clean terminal with good font size
- Slow typing speed for readability
- Highlight important output

### Timing
- Pause briefly after each command
- Allow time for commands to complete
- Don't rush through verification steps
