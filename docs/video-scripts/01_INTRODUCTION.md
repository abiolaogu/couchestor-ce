# Video Training Script: Introduction to CoucheStor

## Video Information

| Field | Value |
|-------|-------|
| Title | Introduction to CoucheStor |
| Duration | 8 minutes |
| Audience | All users |
| Prerequisites | Basic Kubernetes knowledge |

---

## Scene 1: Opening (0:00 - 0:30)

### Visuals
- Animated logo reveal
- Title card: "CoucheStor: Intelligent Storage Tiering"

### Script

> Welcome to the CoucheStor training series. I'm [Presenter Name], and in this video, we'll introduce you to the CoucheStor - an intelligent solution for automated storage tiering in Kubernetes environments running OpenEBS Mayastor.
>
> By the end of this video, you'll understand what the operator does, why it's valuable, and how it fits into your infrastructure.

---

## Scene 2: The Storage Challenge (0:30 - 2:00)

### Visuals
- Animation showing two storage tiers (NVMe = fast/expensive, SATA = slow/cheap)
- Cost comparison graphics
- Diagram of data movement

### Script

> Let's start with the problem we're solving.
>
> In modern data centers, we have different types of storage. On one hand, we have high-performance storage like NVMe - it's blazing fast, but it's expensive. On the other hand, we have SATA or HDD storage - it's much cheaper, but slower.
>
> The challenge is: how do you decide which data goes where?
>
> [Animation: Show hot data on NVMe, cold data on SATA]
>
> Ideally, frequently accessed "hot" data should be on fast NVMe storage for best performance. Meanwhile, rarely accessed "cold" data should be on cheaper SATA storage to save money.
>
> But here's the problem: data access patterns change over time. That database that was super busy last month might be quiet now. That archive everyone forgot about might suddenly become active.
>
> Traditionally, managing this requires constant monitoring, manual analysis, and risky data movement operations. It doesn't scale.

---

## Scene 3: The Solution (2:00 - 3:30)

### Visuals
- Diagram of the operator architecture
- Animation of automated tiering decision

### Script

> This is where the CoucheStor comes in.
>
> [Show architecture diagram]
>
> The CoucheStor automatically monitors your volume performance, makes intelligent tiering decisions, and safely migrates data between storage tiers - all without human intervention.
>
> Think of it as having three capabilities working together:
>
> First, the **Eyes** - it continuously watches volume performance by querying Prometheus for IOPS metrics.
>
> Second, the **Brain** - it analyzes the data and makes decisions based on policies you define. Is this volume hot? Is it cold? Should we move it?
>
> Third, the **Hands** - when a migration is needed, it safely moves data with built-in guarantees that your data is never at risk.
>
> [Animation showing the flow: Metrics → Decision → Migration]
>
> The best part? You define the rules once using simple YAML configuration, and the operator handles everything automatically.

---

## Scene 4: Key Benefits (3:30 - 5:00)

### Visuals
- Bullet points appearing one by one
- Icons for each benefit

### Script

> Let's look at the key benefits:
>
> [Benefit 1 appears]
>
> **Cost Reduction** - By automatically moving cold data to cheaper storage, organizations typically see a 30% reduction in storage costs.
>
> [Benefit 2 appears]
>
> **Operational Efficiency** - No more manual analysis and migration tasks. The operator handles it 24/7, freeing your team for higher-value work.
>
> [Benefit 3 appears]
>
> **Performance Optimization** - Hot data is automatically placed on fast storage, ensuring your applications get the performance they need.
>
> [Benefit 4 appears]
>
> **Data Safety** - This is crucial. The operator is designed with safety as the top priority. Old data is never removed until new data is verified. There's no risk of data loss.
>
> [Benefit 5 appears]
>
> **Kubernetes Native** - If you know Kubernetes, you already know how to use this. It's all standard kubectl commands and YAML files.

---

## Scene 5: How It Works (5:00 - 6:30)

### Visuals
- Step-by-step animation
- Code snippets appearing

### Script

> Let me show you how it works at a high level.
>
> [Step 1 animation]
>
> First, you define a StoragePolicy. This is a Kubernetes custom resource where you specify things like: which storage class to manage, what IOPS thresholds trigger migrations, and which pools to use for hot and cold tiers.
>
> [Show YAML snippet]
>
> Here's a simple example. We're saying: if a volume's IOPS goes above 5000, move it to NVMe. If it drops below 500, move it to SATA.
>
> [Step 2 animation]
>
> Once the policy is applied, the operator starts watching. Every few minutes, it checks each volume's performance, calculates an average, and compares it to your thresholds.
>
> [Step 3 animation]
>
> When a migration is needed, the operator executes a safe 4-phase process: analyze the current state, add a replica on the target storage, wait for data to sync, and only then remove the old replica.
>
> [Step 4 animation]
>
> Throughout all of this, you can see exactly what's happening in the policy status and operator logs.

---

## Scene 6: Use Cases (6:30 - 7:30)

### Visuals
- Icons representing different workloads
- Before/after diagrams

### Script

> Who benefits from this?
>
> [Database icon]
>
> **Database teams** - Production databases stay on fast storage, while development and test databases that are idle can be automatically moved to cheaper tiers.
>
> [Logs icon]
>
> **Log management** - Recent logs need fast access, but older logs can be archived to SATA without manual intervention.
>
> [AI/ML icon]
>
> **Data science workloads** - Training jobs need fast storage, but once complete, the data can be automatically tiered down.
>
> [Enterprise icon]
>
> **Any enterprise with mixed workloads** - If you have varying performance requirements across your applications, the operator ensures optimal placement automatically.

---

## Scene 7: Closing (7:30 - 8:00)

### Visuals
- Summary slide
- Links to next videos

### Script

> To summarize: the CoucheStor brings intelligent, automated storage tiering to your Kubernetes environment. It reduces costs, improves efficiency, and keeps your data safe.
>
> In the next video, we'll walk through the installation process step by step.
>
> If you have questions, check out our documentation or reach out to the platform team.
>
> Thanks for watching!

---

## Post-Production Notes

### B-Roll Suggestions
- Kubernetes dashboard showing pods
- Grafana dashboards with storage metrics
- Terminal showing kubectl commands

### Graphics Needed
- Animated operator logo
- Architecture diagram
- Cost comparison infographic
- 4-phase migration animation

### Accessibility
- Include closed captions
- Ensure color contrast meets WCAG standards
- Describe visual animations in script
