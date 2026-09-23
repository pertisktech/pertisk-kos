import { useCallback, useEffect, useMemo, useState } from 'react'
import { Link, useNavigate } from 'react-router-dom'
import { api } from '../api'
import { Icon } from '../components/Icons'
import PageHeader from '../components/PageHeader'
import StatCard from '../components/StatCard'
import ActivityLog from '../components/ActivityLog'
import { ResourceBars, formatMetric } from '../components/ResourceBars'
import { placeholderSummary, formatK8sVersion } from '../components/ClusterCard'
import { formatProviderKind, providerKindGlyph } from '../components/ClusterMetaBadges'
import { useMgmtRefresh } from '../hooks/useMgmtEvents'
import { readSessionJson, writeSessionJson } from '../utils/sessionCache'

const CACHE_CLUSTERS = 'pertisk_dash_clusters'
const CACHE_PROVIDERS = 'pertisk_dash_providers'
const CACHE_RESOURCES = 'pertisk_dash_resources'
const CACHE_PROVIDER_RES = 'pertisk_dash_provider_res'

function resolveAvailability(...vals) {
  for (const v of vals) {
    if (v === 'online' || v === 'offline') return v
  }
  for (const v of vals) {
    if (v) return v
  }
  return 'unknown'
}

function statusTone(status) {
  if (status === 'ready' || status === 'Healthy') return 'ok'
  if (status === 'error' || status === 'failed' || status === 'degraded') return 'err'
  return 'warn'
}

function availTone(availability) {
  if (availability === 'online') return 'ok'
  if (availability === 'offline') return 'err'
  return 'warn'
}

function placeholderProvider(p) {
  const empty = {
    used: null,
    total: null,
    percent: null,
    unit: '',
    display_used: null,
    display_total: null,
    error: null,
  }
  return {
    provider_id: p.id,
    provider_name: p.name,
    kind: p.kind,
    node: p.node,
    storage: p.storage,
    availability: p.availability || 'unknown',
    cpu: { ...empty, unit: 'cores' },
    memory: { ...empty, unit: 'GiB' },
    disk: { ...empty, unit: 'GiB' },
    error: null,
  }
}

function FleetClusterRow({ summary, onOpen }) {
  const tone = statusTone(summary.status)
  const label = summary.status === 'ready' ? 'healthy' : summary.status || 'unknown'
  const nodes =
    Number(summary.node_count) ||
    (Number(summary.controlplanes) || 0) + (Number(summary.workers) || 0)
  const provider =
    [formatProviderKind(summary.provider_kind), summary.provider_name].filter(Boolean).join(' · ') ||
    '—'
  const ver = formatK8sVersion(summary.k8s_version) || '—'

  return (
    <button type="button" className="fleet-row" onClick={onOpen}>
      <div className="fleet-cell fleet-cell-name">
        <span className={`fleet-dot ${tone}`} aria-hidden />
        <span className="fleet-name">{summary.cluster_name}</span>
      </div>
      <span className="fleet-cell fleet-cell-meta">{provider}</span>
      <span className="fleet-cell">{nodes} nodes</span>
      <span className="fleet-cell">{ver}</span>
      <span className={`fleet-cell fleet-status ${tone}`}>{label}</span>
      <div className="fleet-cell fleet-cell-bars">
        <ResourceBars cpu={summary.cpu} memory={summary.memory} disk={summary.disk} />
      </div>
    </button>
  )
}

function FleetProviderRow({ summary, onOpen }) {
  const avail = summary.availability || 'unknown'
  const tone = availTone(avail)
  const label = avail === 'online' ? 'online' : avail === 'offline' ? 'offline' : 'unknown'
  const meta = [formatProviderKind(summary.kind), summary.node, summary.storage]
    .filter(Boolean)
    .join(' · ')

  return (
    <button type="button" className="fleet-row fleet-row-provider" onClick={onOpen}>
      <div className="fleet-cell fleet-cell-name">
        <span className={`fleet-mark ${tone}`} aria-hidden>
          {providerKindGlyph(summary.kind)}
        </span>
        <div className="fleet-name-stack">
          <span className="fleet-name">{summary.provider_name}</span>
          <span className="fleet-sub">{meta || formatProviderKind(summary.kind)}</span>
        </div>
      </div>
      <span className={`fleet-cell fleet-status ${tone}`}>{label}</span>
      <span className="fleet-cell fleet-mono" title={formatMetric(summary.cpu)}>
        {formatMetric(summary.cpu)}
      </span>
      <span className="fleet-cell fleet-mono" title={formatMetric(summary.memory)}>
        {formatMetric(summary.memory)}
      </span>
      <span className="fleet-cell fleet-mono" title={formatMetric(summary.disk)}>
        {formatMetric(summary.disk)}
      </span>
      <div className="fleet-cell fleet-cell-bars">
        <ResourceBars cpu={summary.cpu} memory={summary.memory} disk={summary.disk} />
      </div>
    </button>
  )
}

export default function Dashboard() {
  const nav = useNavigate()
  const [clusters, setClusters] = useState(() => readSessionJson(CACHE_CLUSTERS, []))
  const [providers, setProviders] = useState(() => readSessionJson(CACHE_PROVIDERS, []))
  const [resources, setResources] = useState(() => readSessionJson(CACHE_RESOURCES, []))
  const [providerRes, setProviderRes] = useState(() => readSessionJson(CACHE_PROVIDER_RES, []))
  const [listLoading, setListLoading] = useState(() => {
    const cached = readSessionJson(CACHE_CLUSTERS, null)
    return !Array.isArray(cached)
  })
  const [resourcesErr, setResourcesErr] = useState('')
  const [resourcesLoading, setResourcesLoading] = useState(false)
  const [filter, setFilter] = useState('')

  const load = useCallback(() => {
    Promise.all([
      api('/clusters').catch(() => []),
      api('/providers').catch(() => []),
    ]).then(([c, p]) => {
      const nextClusters = Array.isArray(c) ? c : []
      const nextProviders = Array.isArray(p) ? p : []
      setClusters(nextClusters)
      setProviders(nextProviders)
      writeSessionJson(CACHE_CLUSTERS, nextClusters)
      writeSessionJson(CACHE_PROVIDERS, nextProviders)
      setListLoading(false)
    })
  }, [])

  const loadResources = useCallback(() => {
    setResourcesLoading(true)
    api('/dashboard/resources')
      .then((rows) => {
        if (Array.isArray(rows)) {
          setResources(rows)
          writeSessionJson(CACHE_RESOURCES, rows)
        }
        setResourcesErr('')
      })
      .catch((e) => {
        const msg = e.message || 'failed to load resources'
        if (/failed to fetch|networkerror|load failed|sending request/i.test(msg)) {
          setResourcesErr('Cannot reach management API at :8080 — is pertisk-mgmt running?')
        } else {
          setResourcesErr(msg)
        }
      })
      .finally(() => setResourcesLoading(false))
  }, [])

  const loadProviderResources = useCallback(() => {
    api('/dashboard/providers')
      .then((rows) => {
        if (Array.isArray(rows)) {
          setProviderRes(rows)
          writeSessionJson(CACHE_PROVIDER_RES, rows)
        }
      })
      .catch(() => {})
  }, [])

  useEffect(() => {
    load()
    loadResources()
    loadProviderResources()
  }, [load, loadResources, loadProviderResources])

  useMgmtRefresh(() => {
    load()
    loadResources()
    loadProviderResources()
  })

  const displayResources = useMemo(() => {
    if (clusters.length === 0) return []
    const byId = new Map(resources.map((r) => [r.cluster_id, r]))
    return clusters.map((c) => {
      const live = byId.get(c.id) || placeholderSummary(c)
      return {
        ...live,
        provider_kind: c.provider_kind,
        provider_name: c.provider_name,
        arch: c.arch,
        vip: c.vip,
        controlplanes: c.controlplanes,
        workers: c.workers,
        k8s_version: c.k8s_version,
        availability: resolveAvailability(c.availability, live.availability),
      }
    })
  }, [resources, clusters])

  const filteredClusters = useMemo(() => {
    const q = filter.trim().toLowerCase()
    if (!q) return displayResources
    return displayResources.filter((s) => {
      const hay = [
        s.cluster_name,
        s.provider_name,
        s.provider_kind,
        s.status,
        formatK8sVersion(s.k8s_version),
      ]
        .filter(Boolean)
        .join(' ')
        .toLowerCase()
      return hay.includes(q)
    })
  }, [displayResources, filter])

  const ready = clusters.filter((c) => c.status === 'ready').length
  const cps = clusters.reduce((n, c) => n + (c.controlplanes || 0), 0)
  const wks = clusters.reduce((n, c) => n + (c.workers || 0), 0)
  const totalNodes = cps + wks
  const providersOnline = providers.filter((p) => p.availability === 'online').length
  const attention = clusters.filter((c) =>
    ['error', 'degraded', 'failed', 'provisioning'].includes(c.status),
  ).length
  const dashNum = listLoading && clusters.length === 0
  const provisioning = clusters.filter((c) => c.status === 'provisioning').length
  const providerKinds = new Set(providers.map((p) => p.kind).filter(Boolean)).size

  const displayProviders = useMemo(() => {
    if (providers.length === 0) return []
    const byId = new Map(providerRes.map((r) => [r.provider_id, r]))
    return providers.map((p) => {
      const live = byId.get(p.id) || placeholderProvider(p)
      return {
        ...live,
        availability: resolveAvailability(p.availability, live.availability),
      }
    })
  }, [providerRes, providers])

  const systemStatus = [
    {
      label: 'API server',
      ok: !resourcesErr,
      text: resourcesErr ? 'Unreachable' : 'Operational',
    },
    {
      label: 'Clusters ready',
      ok: ready === clusters.length && clusters.length > 0,
      text: dashNum ? '—' : `${ready} / ${clusters.length || 0}`,
    },
    {
      label: 'Provider connection',
      ok: providersOnline > 0 || providers.length === 0,
      text: dashNum ? '—' : `${providersOnline} of ${providers.length} online`,
    },
    {
      label: 'Attention needed',
      ok: attention === 0,
      text: dashNum ? '—' : attention === 0 ? 'None' : `${attention} cluster${attention === 1 ? '' : 's'}`,
    },
  ]

  return (
    <div className="dash-page">
      <PageHeader
        title="Fleet overview"
        description={
          dashNum
            ? 'Loading your Kubernetes infrastructure…'
            : `${clusters.length} cluster${clusters.length === 1 ? '' : 's'} across ${providerKinds || providers.length} hypervisor target${(providerKinds || providers.length) === 1 ? '' : 's'}.`
        }
        actions={
          <>
            <button
              type="button"
              className="secondary btn-icon"
              onClick={() => {
                load()
                loadResources()
                loadProviderResources()
              }}
              disabled={resourcesLoading}
            >
              <Icon name="refresh" size={16} /> Refresh
            </button>
            <Link className="btn btn-icon" to="/clusters?new=1">
              <Icon name="plus" size={16} /> New cluster
            </Link>
          </>
        }
      />

      <section className="stat-grid">
        <StatCard
          icon="clusters"
          label="Clusters"
          value={dashNum ? '—' : clusters.length}
          hint={dashNum ? undefined : `${ready} healthy`}
          hintTone="ok"
        />
        <StatCard
          icon="machines"
          label="Nodes"
          value={dashNum ? '—' : totalNodes}
          hint={
            dashNum
              ? undefined
              : `${Math.max(0, totalNodes - provisioning)} ready${provisioning ? ` · ${provisioning} provisioning` : ''}`
          }
        />
        <StatCard
          icon="providers"
          label="Providers"
          value={dashNum ? '—' : providers.length}
          hint={dashNum ? undefined : `${providersOnline} online`}
        />
        <StatCard
          icon="alert"
          label="Attention"
          value={dashNum ? '—' : attention}
          valueTone={attention > 0 ? 'warn' : undefined}
          hint={
            dashNum
              ? undefined
              : attention === 0
                ? 'All clear'
                : provisioning
                  ? 'Provisioning / degraded'
                  : 'Needs review'
          }
        />
      </section>

      <div className="dash-main-grid">
        <section className="fleet-panel">
          <div className="fleet-panel-head">
            <div className="fleet-panel-title-row">
              <h2 className="fleet-panel-title">Clusters</h2>
              <Link to="/clusters" className="section-link">
                View all
              </Link>
            </div>
            <div className="fleet-filter">
              <Icon name="search" size={14} />
              <input
                type="search"
                value={filter}
                onChange={(e) => setFilter(e.target.value)}
                placeholder="Filter…"
                aria-label="Filter clusters"
              />
            </div>
          </div>
          {resourcesErr && <div className="error fleet-panel-err">{resourcesErr}</div>}
          {listLoading && clusters.length === 0 ? (
            <div className="fleet-empty muted">Loading clusters…</div>
          ) : clusters.length === 0 ? (
            <div className="fleet-empty">
              <p className="muted" style={{ margin: 0 }}>
                No clusters yet. Add a provider, then create control planes and workers.
              </p>
              <div className="dash-empty-actions">
                <Link className="btn btn-icon" to="/providers">
                  <Icon name="providers" size={16} /> Providers
                </Link>
                <Link className="btn btn-icon" to="/clusters?new=1">
                  <Icon name="plus" size={16} /> New cluster
                </Link>
              </div>
            </div>
          ) : filteredClusters.length === 0 ? (
            <div className="fleet-empty muted">No clusters match this filter.</div>
          ) : (
            <div className="fleet-scroll">
              <div className="fleet-table fleet-table-clusters">
                <div className="fleet-head" aria-hidden>
                  <span>Cluster</span>
                  <span>Provider</span>
                  <span>Nodes</span>
                  <span>Version</span>
                  <span>Status</span>
                  <span>Resources</span>
                </div>
                <div className="fleet-body">
                  {filteredClusters.map((s) => (
                    <FleetClusterRow
                      key={s.cluster_id}
                      summary={s}
                      onOpen={() => nav(`/clusters/${s.cluster_id}`)}
                    />
                  ))}
                </div>
              </div>
            </div>
          )}
        </section>

        <div className="dash-side-stack">
          <ActivityLog />
          <section className="fleet-side-panel sys-status-panel">
            <div className="fleet-panel-head">
              <h2 className="fleet-panel-title">System status</h2>
            </div>
            <div className="sys-status-list">
              {systemStatus.map((item) => (
                <div key={item.label} className="sys-status-row">
                  <span className="sys-status-label">{item.label}</span>
                  <span className={`sys-status-value ${item.ok ? 'ok' : 'warn'}`}>
                    <span className="dot" aria-hidden />
                    {item.text}
                  </span>
                </div>
              ))}
            </div>
          </section>
        </div>
      </div>

      <section className="fleet-panel">
        <div className="fleet-panel-head">
          <div className="fleet-panel-title-row">
            <h2 className="fleet-panel-title">Providers</h2>
            <Link to="/providers" className="section-link">
              All providers
            </Link>
          </div>
          {providers.length > 0 && (
            <p className="fleet-panel-sub muted">
              {providersOnline} of {providers.length} hypervisors online
            </p>
          )}
        </div>
        {providers.length === 0 ? (
          <div className="fleet-empty">
            <p className="muted" style={{ margin: 0 }}>
              No providers yet. Add Proxmox, vSphere, Nutanix, or Pertisk VMs.
            </p>
            <div className="dash-empty-actions">
              <Link className="btn btn-icon" to="/providers">
                <Icon name="plus" size={16} /> Add provider
              </Link>
            </div>
          </div>
        ) : (
          <div className="fleet-scroll">
            <div className="fleet-table fleet-table-providers">
              <div className="fleet-head" aria-hidden>
                <span>Provider</span>
                <span>Status</span>
                <span>CPU</span>
                <span>Memory</span>
                <span>Disk</span>
                <span>Resources</span>
              </div>
              <div className="fleet-body">
                {displayProviders.map((s) => (
                  <FleetProviderRow
                    key={s.provider_id}
                    summary={s}
                    onOpen={() => nav(`/providers/${s.provider_id}`)}
                  />
                ))}
              </div>
            </div>
          </div>
        )}
      </section>
    </div>
  )
}
