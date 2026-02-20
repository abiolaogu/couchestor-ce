# Figma/Make Design Prompts — Couchestor CE
> Version: 1.0 | Last Updated: 2026-02-18 | Status: Draft
> Classification: Internal | Author: AIDD System

## 1. Overview

This document provides structured design prompts for generating UI/UX mockups, architecture diagrams, workflow visuals, and marketing materials for CoucheStor Community Edition using Figma, Make (Integromat), and AI-assisted design tools. Each prompt includes context, visual requirements, and acceptance criteria.

## 2. Architecture Diagrams

### 2.1 Prompt: Eyes-Brain-Hands Architecture Diagram

**Context**: CoucheStor uses a three-component architecture pattern called "Eyes, Brain, Hands" for intelligent storage tiering in Kubernetes.

**Prompt**:
```
Create a technical architecture diagram for a Kubernetes storage operator with three main components arranged left-to-right:

1. "Metrics Watcher" (labeled "Eyes") — represented as a sensor/eye icon
   - Connects downward to "Prometheus" (database icon)
   - Shows data flow: "IOPS Metrics" on the connection line

2. "Controller" (labeled "Brain") — represented as a brain/CPU icon
   - Receives input from Metrics Watcher via arrow labeled "HeatScore"
   - Connects downward to "StoragePolicy CRD" (document icon)
   - Contains sub-label: "Reconciliation Loop"

3. "Migrator" (labeled "Hands") — represented as a hand/gear icon
   - Receives commands from Controller via arrow labeled "Migration Request"
   - Connects downward to three storage blocks:
     a. "Hot Tier (NVMe)" — red/orange color
     b. "Warm Tier (SSD)" — yellow color
     c. "Cold Tier (HDD)" — blue color

Style: Clean, modern, flat design. Dark background (#1a1a2e) with white text and accent colors. Use rounded rectangles for components. Include a subtle Kubernetes cluster boundary around all components.

Dimensions: 1920x1080px
```

**Acceptance Criteria**:
- All three components clearly labeled with both technical and metaphor names
- Data flow direction is unambiguous (left to right)
- Storage tiers visually distinct with color coding
- Kubernetes cluster boundary visible
- Text legible at 50% zoom

### 2.2 Prompt: Erasure Coding Visual

**Prompt**:
```
Create an infographic explaining Reed-Solomon erasure coding (4+2 configuration):

Top section: "Original Data" — show a single block labeled "100 GB"
Middle section: Show the data splitting into 6 equal blocks arranged horizontally:
  - 4 blocks colored blue, labeled "Data Shard 1" through "Data Shard 4" (each ~25 GB)
  - 2 blocks colored green, labeled "Parity Shard 1" and "Parity Shard 2"
  - Red X marks on any 2 shards with text: "Tolerates any 2 failures"

Bottom section: Comparison table:
  - "3-way Replication: 300 GB total (200% overhead)"
  - "4+2 Erasure Coding: 150 GB total (50% overhead)"
  - Large text: "Save 150 GB (50% less storage)"

Style: Clean infographic style, light background, bold typography.
Brand colors: Primary blue (#2563eb), accent green (#16a34a), warning red (#dc2626).
Dimensions: 1200x800px
```

### 2.3 Prompt: Three-Tiered Cache Diagram

**Prompt**:
```
Create a layered cache architecture diagram showing three tiers:

Layer 1 (top, smallest): "L1 Cache — RAM"
  - Capacity: 50 GB
  - Latency: < 1 microsecond
  - Icon: RAM chip
  - Color: Hot red/orange gradient

Layer 2 (middle): "L2 Cache — NVMe"
  - Capacity: 500 GB
  - Latency: < 100 microseconds
  - Icon: SSD drive
  - Color: Warm yellow gradient

Layer 3 (bottom, largest): "L3 Cache — Cold Storage"
  - Capacity: 10 TB+
  - Latency: < 10 milliseconds
  - Icon: HDD stack
  - Color: Cool blue gradient

Show arrows between layers labeled "Promote on hit" (upward) and "Evict on full" (downward).
Include a "Cache Manager" component above L1 with arrows going down through all layers.
Show a "Cache Miss" path that goes through all three layers to "Backend Storage".

Style: Pyramid/funnel layout. Performance decreases downward, capacity increases downward.
Dimensions: 1080x1080px (square for social media)
```

## 3. Workflow Diagrams

### 3.1 Prompt: 4-Step Migration Protocol

**Prompt**:
```
Create a step-by-step workflow diagram for a volume migration protocol with 4 phases:

Phase 1: "ANALYZE" (color: blue)
  - Icon: magnifying glass
  - Checklist items: "Volume Online?", "Cooldown expired?", "Capacity available?"
  - Output: Go/No-Go decision

Phase 2: "SCALE UP" (color: green)
  - Icon: plus symbol
  - Action: "Add replica on target tier"
  - Show: Old replica (source) + New replica (target) both exist

Phase 3: "WAIT SYNC" (color: yellow)
  - Icon: sync/refresh
  - Action: "Poll every 10s until synced"
  - Timer: "Timeout: 30 minutes"
  - Show: Data flowing from old to new replica

Phase 4: "SCALE DOWN" (color: orange)
  - Icon: minus symbol
  - Action: "Remove old replica"
  - Show: Only new replica remains
  - Footnote: "Skipped in Preservation Mode"

Include a "FAILURE" path from any phase back to "ABORT — Original data preserved"
in red, emphasizing zero data loss guarantee.

Layout: Horizontal flow, left to right. Each phase in a distinct card/box.
Dimensions: 1920x600px (wide banner format)
```

### 3.2 Prompt: Reconciliation Loop Flowchart

**Prompt**:
```
Create a flowchart for a Kubernetes operator reconciliation loop:

Start: "StoragePolicy Event" (CRD created/updated)
  │
  ▼
Decision: "Policy enabled?" — No → "Set phase=Disabled, return"
  │ Yes
  ▼
Process: "List PVs matching StorageClass"
  │
  ▼
Process: "Filter by volumeSelector labels"
  │
  ▼
Loop: "For each volume:"
  │
  ├── Process: "Query Prometheus for IOPS"
  │
  ├── Decision: "Classify tier (Hot/Warm/Cold)"
  │
  ├── Decision: "Tier change needed?"
  │     │ No → continue loop
  │     │ Yes
  │     ▼
  ├── Decision: "Cooldown expired?" — No → continue loop
  │     │ Yes
  │     ▼
  ├── Decision: "Migration slots available?" — No → continue loop
  │     │ Yes
  │     ▼
  └── Process: "Spawn migration task"
  │
  ▼
Process: "Update StoragePolicy status"
  │
  ▼
End: "Requeue after 60 seconds"

Style: Standard flowchart with diamond decisions, rounded process boxes.
Colors: Blue for process, yellow for decision, green for start/end.
Dimensions: 800x1200px (portrait)
```

## 4. Dashboard Mockups

### 4.1 Prompt: Grafana Monitoring Dashboard

**Prompt**:
```
Design a Grafana-style monitoring dashboard for a storage tiering operator with the following panels:

Row 1 (Overview):
  - Panel 1: "Watched Volumes" — single stat, large number, green
  - Panel 2: "Active Migrations" — single stat, number with gauge
  - Panel 3: "Migration Success Rate" — single stat, percentage, green/red threshold
  - Panel 4: "EC Degraded Stripes" — single stat, number, red if > 0

Row 2 (Volume Distribution):
  - Panel 5: "Volumes by Tier" — pie chart (Hot=red, Warm=yellow, Cold=blue)
  - Panel 6: "Migrations Over Time" — time series graph, stacked area

Row 3 (Performance):
  - Panel 7: "Cache Hit Ratio" — time series, three lines (L1, L2, L3)
  - Panel 8: "Migration Duration" — histogram/heatmap

Row 4 (Erasure Coding):
  - Panel 9: "EC Stripes Total vs Degraded" — time series, two lines
  - Panel 10: "Reconstruction Activity" — time series bar chart

Style: Dark Grafana theme. Time range selector at top. Auto-refresh indicator.
Dimensions: 1920x1080px
```

### 4.2 Prompt: Admin Console Mockup

**Prompt**:
```
Design a web admin console for CoucheStor with:

Navigation sidebar (left):
  - Dashboard (home icon)
  - Storage Policies (list icon)
  - Erasure Coding (shield icon)
  - Volumes (database icon)
  - Monitoring (chart icon)
  - Settings (gear icon)

Main content area showing "Storage Policies" page:
  - Header: "Storage Policies" with "Create Policy" button (blue)
  - Table with columns: Name, Status, Watched Volumes, Hot, Warm, Cold, Active Migrations, Last Reconcile
  - Sample rows with realistic data
  - Status badges: "Active" (green), "Disabled" (gray), "Error" (red)
  - Click row to expand details

Style: Modern SaaS admin panel. Clean white background, subtle shadows.
Typography: Inter or system font. Blue primary color (#2563eb).
Dimensions: 1440x900px
```

## 5. Marketing & Documentation Visuals

### 5.1 Prompt: Feature Comparison Table

**Prompt**:
```
Create a visual comparison table: "CoucheStor CE vs Traditional Storage"

Headers: Feature | Traditional | CoucheStor CE
Rows:
  - Tiering: "Manual" (red X) | "Automated IOPS-based" (green check)
  - Cold Storage Protection: "3-way replication (200% overhead)" | "Erasure Coding (50% overhead)" (green check)
  - Migration Safety: "Risk of data loss" (red X) | "4-step zero-loss protocol" (green check)
  - Cost Optimization: "Static allocation" | "30-50% savings" (green check)
  - Observability: "Basic logs" | "Prometheus metrics + health probes" (green check)
  - Configuration: "Scripts and manual" | "Kubernetes CRDs" (green check)

Style: Clean comparison table with alternating row backgrounds.
Highlight CoucheStor column with subtle blue background.
Dimensions: 1200x600px
```

### 5.2 Prompt: Cost Savings Infographic

**Prompt**:
```
Create an infographic showing storage cost savings with CoucheStor:

Scenario: "100 TB Mixed Workload — 12 Month Projection"

Without CoucheStor:
  - All data on NVMe: 100 TB x $0.35/GB/month = $35,000/month
  - Annual: $420,000
  - Visual: Large red bar

With CoucheStor:
  - Hot (20 TB NVMe): $7,000/month
  - Warm (30 TB SSD): $3,000/month
  - Cold (50 TB HDD + EC): $1,500/month
  - Total: $11,500/month
  - Annual: $138,000
  - Visual: Three smaller colored bars (red, yellow, blue)

Savings callout: "$282,000 saved annually (67% reduction)"
Large green badge with savings amount.

Style: Financial infographic, professional, data-driven.
Dimensions: 1200x800px
```

## 6. Make (Integromat) Automation Workflows

### 6.1 Prompt: Documentation Generation Pipeline

**Purpose**: Automate document generation when code changes are pushed.

```
Make Workflow: "AIDD Documentation Pipeline"

Trigger: GitHub webhook (push to main branch)
  │
  ▼
Step 1: Filter — Check if docs/ or src/ files changed
  │
  ▼
Step 2: HTTP Request — Fetch latest CLAUDE.md from repo
  │
  ▼
Step 3: AI Module — Generate/update documentation based on code changes
  │
  ▼
Step 4: GitHub — Create PR with updated docs
  │
  ▼
Step 5: Slack Notification — "Documentation PR created: #{pr_number}"
```

### 6.2 Prompt: Release Notes Automation

```
Make Workflow: "Release Notes Generator"

Trigger: GitHub Release created
  │
  ▼
Step 1: GitHub — Fetch all commits since last release tag
  │
  ▼
Step 2: GitHub — Fetch all merged PRs since last release
  │
  ▼
Step 3: AI Module — Categorize changes (features, fixes, breaking)
  │
  ▼
Step 4: AI Module — Generate release notes markdown
  │
  ▼
Step 5: GitHub — Update release description with generated notes
  │
  ▼
Step 6: Slack — Post release announcement to #engineering channel
```

## 7. Design System Tokens

### 7.1 Color Palette
| Token | Hex | Usage |
|-------|-----|-------|
| `--color-hot` | #ef4444 | Hot tier indicators |
| `--color-warm` | #f59e0b | Warm tier indicators |
| `--color-cold` | #3b82f6 | Cold tier indicators |
| `--color-primary` | #2563eb | Primary actions, links |
| `--color-success` | #16a34a | Success states, healthy |
| `--color-warning` | #f59e0b | Warning states, degraded |
| `--color-error` | #dc2626 | Error states, critical |
| `--color-bg-dark` | #1a1a2e | Dark mode background |
| `--color-bg-light` | #ffffff | Light mode background |
| `--color-text` | #1f2937 | Primary text |
| `--color-text-muted` | #6b7280 | Secondary text |

### 7.2 Typography
| Element | Font | Size | Weight |
|---------|------|------|--------|
| H1 | Inter | 32px | 700 |
| H2 | Inter | 24px | 600 |
| H3 | Inter | 20px | 600 |
| Body | Inter | 16px | 400 |
| Code | JetBrains Mono | 14px | 400 |
| Label | Inter | 12px | 500 |

### 7.3 Spacing Scale
| Token | Value |
|-------|-------|
| `--space-xs` | 4px |
| `--space-sm` | 8px |
| `--space-md` | 16px |
| `--space-lg` | 24px |
| `--space-xl` | 32px |
| `--space-2xl` | 48px |

## 8. Icon Requirements

| Icon | Context | Style |
|------|---------|-------|
| Eye | Metrics Watcher component | Outline, 24px |
| Brain | Controller component | Outline, 24px |
| Hand | Migrator component | Outline, 24px |
| NVMe Drive | Hot tier | Filled, 32px |
| SSD Drive | Warm tier | Filled, 32px |
| HDD Drive | Cold tier | Filled, 32px |
| Shield | Erasure coding | Outline, 24px |
| Chart | Metrics/monitoring | Outline, 24px |
| Gear | Configuration | Outline, 24px |
| Kubernetes | Platform context | Official logo, 24px |

---

*End of Figma/Make Design Prompts*
