# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

**Inferred from product docs and roles (`admin` / `operator` / `viewer`):** primary users are platform / SRE / infrastructure operators who provision and run Kubernetes on private hypervisors (Proxmox, ESXi, Nutanix AHV, Pertisk VMs). Day-to-day job: register hypervisors, create and operate HA clusters, watch fleet health and activity, run kubectl / pertiskctl from the management host — without SSH into guest nodes.

## Product Purpose

Pertisk KOS is an immutable, API-only Kubernetes node OS (`pertiskd` as PID 1, `pertiskctl`, containerd + kubelet; no SSH in production images) plus an optional management plane (`pertisk-mgmt` API + React UI) that provisions and operates fleets on those hypervisors. Success means operators can bootstrap HA clusters, upgrade via signed A/B images, and manage nodes only through typed APIs and the fleet UI.

## Positioning

**Confirmed claim:** Kubernetes node OS you operate only through a typed API (no SSH), with a fleet UI that provisions HA clusters on your own hypervisors. Neighboring "cluster UI" products that assume SSH-able guests or mutable node OS cannot truthfully copy that mechanism.

## Operating Context

- Management host runs `pertisk-mgmt` (single-port API + UI) and talks to hypervisors; guests are immutable Pertisk images.
- Operators use dashboard / clusters / providers / machines, cluster shell dock (kubectl + pertiskctl), audit activity, add-ons, Terraform provider for the same API.
- Serial console dashboard on guests; mTLS management traffic; Auth0 or local auth with role claims.

## Capabilities and Constraints

- Guests: no SSH in production; immutable root; signed OS upgrades; A/B slots.
- Mgmt: Proxmox, standalone ESXi, Nutanix AHV, Pertisk VMs providers; HA (stacked etcd + kube-vip); CNI options; audit log; machines inventory; config templates; join/adopt nodes.
- **Undecided / later:** multi-tenant orgs / SaaS packaging (Phase D3); Cluster API provider (CAPx) planned; public-cloud providers paused.
- Terminology: cluster lifecycle (`ready`, provisioning, …) is separate from live availability (`online` / `offline`).

## Brand Commitments

- Product name: **Pertisk KOS**; UI brand mark commonly rendered as `Pertisk KOS`.
- Assets: `web/mgmt-ui/public/logo.svg`, `web/mgmt-ui/public/favicon.svg`, `BrandLogo` component.
- **User-pinned for redesign (2026-09-23):** clean, minimal, modern web-infrastructure UI — quiet density, clear hierarchy, infrastructure-console clarity over decorative chrome.
- **Redesign scope (2026-09-23):** whole management shell (sidebar, topbar, dashboard, clusters, providers, detail, bottom shell) — not dashboard-only.
- **Redesign constraints (2026-09-23):** look and layout only — preserve routes, copy, wizards, and data/behavior; no IA/nav/copy rewrite.
- **Anti-goals (2026-09-23):** no neon / glow, no playful gamification, no card clutter — keep quiet ops density.

## Evidence on Hand

- Architecture and milestones: root `DESIGN.md` (product/engineering draft — not a visual design system).
- Operator docs: `docs/MGMT.md`, `docs/DEPLOY.md`, README.
- UI implementation: `web/mgmt-ui` (React + Vite).
- Prior visual exploration (reference only): `/Users/nat/projects/pertisk-tech/designs/pertisk-kos-redesign`.
- Do not fabricate customers, benchmarks, pricing, or SaaS claims.

## Product Principles

1. **API-only ops** — Prefer typed management over shell access; guests stay sealed.
2. **Fleet truth over chrome** — Status, reachability, and jobs must stay legible under any visual redesign.
3. **Operator speed** — Common paths (new cluster, shell, refresh, audit) stay short and scannable.
4. **Honest state** — Lifecycle and live online/offline are never collapsed into a single misleading badge.
5. **Minimal surface** — Clean modern infrastructure UI; every element earns its place.

## Accessibility & Inclusion

No product-specific WCAG target recorded yet. Default expectation for operator web UI: keyboard-reachable primary actions, visible focus, sufficient contrast, no information by color alone.
