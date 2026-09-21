import { useCallback, useEffect, useMemo, useState } from 'react'
import { Link, useNavigate } from 'react-router-dom'
import { api } from '../api'
import { Icon } from '../components/Icons'
import PageHeader from '../components/PageHeader'
import StatCard from '../components/StatCard'
import { placeholderSummary, formatK8sVersion } from '../components/ClusterCard'
import { formatProviderKind, providerKindGlyph } from '../components/ClusterMetaBadges'
import { useMgmtRefresh } from '../hooks/useMgmtEvents'
import { readSessionJson, writeSessionJson } from '../utils/sessionCache'

const CACHE_CLUSTERS = 'pertisk_dash_clusters'
const CACHE_PROVIDERS = 'pertisk_dash_providers'
const CACHE_RESOURCES = 'pertisk_dash_resources'
const CACHE_PROVIDER_RES = 'pertisk_dash_provider_res'

/** Prefer definitive online/offline; never let a stale "unknown" hide a known status. */
function resolveAvailability(...vals) {
  for (const v of vals) {
    if (v === 'online' || v === 'offline') return v
  }
  for (const v of vals) {
    if (v) return v
  }
  return 'unknown'
}

function formatMetric(m) {
  if (!m) return '—'
  const used = m.display_used ?? m.used
  const total = m.display_total ?? m.total
  if (used == null && total == null) return '—'
  if (typeof used === 'string' && typeof total === 'string') {
    const um = used.trim().match(/^([\d.]+)\s*(.*)$/)
    const tm = total.trim().match(/^([\d.]+)\s*(.*)$/)
    if (um && tm && um[2] && um[2] === tm[2]) {
      return `${um[1]} / ${tm[1]} ${tm[2]}`.trim()
    }
  }
  const unit = m.display_used == null && m.display_total == null && m.unit ? ` ${m.unit}` : ''
  if (total == null) return `${used}${unit}`
  if (used == null) return `— / ${total}${unit}`
  return `${used} / ${total}${unit}`
}

function MetricBoxes({ summary }) {
  return (
    <div className="live-metric-row">
      <div className="live-metric-box">
        <div className="live-metric-label">
          <Icon name="cpu" size={12} /> CPU
        </div>
        <div className="live-metric-value" title={formatMetric(summary.cpu)}>
          {formatMetric(summary.cpu)}
        </div>
      </div>
      <div className="live-metric-box">
        <div className="live-metric-label">
          <Icon name="memory" size={12} /> Memory
        </div>
        <div className="live-metric-value" title={formatMetric(summary.memory)}>
          {formatMetric(summary.memory)}
        </div>
      </div>
      <div className="live-metric-box">
        <div className="live-metric-label">
          <Icon name="disk" size={12} /> Disk
        </div>
        <div className="live-metric-value" title={formatMetric(summary.disk)}>
          {formatMetric(summary.disk)}
        </div>
      </div>
    </div>
  )
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

function ProviderResourceCard({ summary, onOpen }) {
  const avail = summary.availability || 'unknown'
  const tone = availTone(avail)
  const label =
    avail === 'online' ? 'Connected' : avail === 'offline' ? 'Offline' : 'Unknown'
  const sub = [
    formatProviderKind(summary.kind),
    summary.node,
    summary.storage,
  ]
    .filter(Boolean)
    .join(' · ')

  return (
    <article
      className="live-resource-card"
      role="link"
      tabIndex={0}
      onClick={onOpen}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault()
          onOpen()
        }
      }}
    >
      <div className="live-resource-head">
        <div className="live-resource-title">
          <span className={`live-resource-icon ${tone === 'ok' ? 'ok' : 'warn'}`} aria-hidden>
            {providerKindGlyph(summary.kind)}
          </span>
          <div style={{ minWidth: 0 }}>
            <p className="live-resource-name">{summary.provider_name}</p>
            <p className="live-resource-sub" title={sub}>
              {sub || `${formatProviderKind(summary.kind)} · CPU · memory · disk`}
            </p>
          </div>
        </div>
        <span className={`live-resource-status ${tone === 'ok' ? 'ok' : 'warn'}`}>
          <span className="dot" aria-hidden />
          {label}
        </span>
      </div>
      <MetricBoxes summary={summary} />
      {summary.error && avail !== 'offline' && (
        <p className="muted cluster-resource-soft-err" title={summary.error}>
          <Icon name="alert" size={12} />
          {summary.error}
        </p>
      )}
    </article>
  )
}

function LiveClusterResourceCard({ summary, onOpen }) {
  const tone = statusTone(summary.status)
  const label = summary.status === 'ready' ? 'Healthy' : (summary.status || 'Unknown')

  return (
    <article
      className="live-resource-card"
      role="link"
      tabIndex={0}
      onClick={onOpen}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault()
          onOpen()
        }
      }}
    >
      <div className="live-resource-head">
        <div className="live-resource-title">
          <span className={`live-resource-icon ${tone === 'ok' ? 'ok' : 'warn'}`} aria-hidden>
            <Icon name="clusters" size={16} />
          </span>
          <div style={{ minWidth: 0 }}>
            <p className="live-resource-name">{summary.cluster_name}</p>
            <p className="live-resource-sub">CPU · memory · disk usage</p>
          </div>
        </div>
        <span className={`live-resource-status ${tone === 'ok' ? 'ok' : 'warn'}`}>
          <span className="dot" aria-hidden />
          {label}
        </span>
      </div>
      <MetricBoxes summary={summary} />
    </article>
  )
}

function placeholderProvider(p) {
  const empty = { used: null, total: null, percent: null, unit: '', display_used: null, display_total: null, error: null }
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

  const load = useCallback(() => {
    Promise.all([
      api('/clusters').catch(() => []),
      api('/providers').catch(() => []),
    ]).then(([c, p]) => {
      const clusters = Array.isArray(c) ? c : []
      const providers = Array.isArray(p) ? p : []
      setClusters(clusters)
      setProviders(providers)
      writeSessionJson(CACHE_CLUSTERS, clusters)
      writeSessionJson(CACHE_PROVIDERS, providers)
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
          setResourcesErr(
            'Cannot reach management API at :8080 — is pertisk-mgmt running?',
          )
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
    return clusters
      .map((c) => {
        const live = byId.get(c.id) || placeholderSummary(c)
        return {
          ...live,
          provider_kind: c.provider_kind,
          provider_name: c.provider_name,
          arch: c.arch,
          vip: c.vip,
          controlplanes: c.controlplanes,
          workers: c.workers,
          availability: resolveAvailability(c.availability, live.availability),
        }
      })
      .filter((s) => s.availability === 'online')
  }, [resources, clusters])

  const ready = clusters.filter((c) => c.status === 'ready').length
  const cps = clusters.reduce((n, c) => n + (c.controlplanes || 0), 0)
  const wks = clusters.reduce((n, c) => n + (c.workers || 0), 0)
  const totalNodes = cps + wks
  const providersOnline = providers.filter((p) => p.availability === 'online').length
  const attention = clusters.filter(
    (c) => c.status === 'error' || c.status === 'degraded' || c.status === 'failed' || c.status === 'provisioning',
  ).length
  const recent = clusters.slice(0, 8)
  const dashNum = listLoading && clusters.length === 0
  const provisioning = clusters.filter((c) => c.status === 'provisioning').length

  const displayProviders = useMemo(() => {
    if (providers.length === 0) return []
    const byId = new Map(providerRes.map((r) => [r.provider_id, r]))
    return providers
      .map((p) => {
        const live = byId.get(p.id) || placeholderProvider(p)
        return {
          ...live,
          availability: resolveAvailability(p.availability, live.availability),
        }
      })
      .filter((s) => s.availability === 'online')
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
        title="Dashboard"
        description="Overview of your Kubernetes infrastructure."
        actions={
          <>
            <button type="button" className="secondary btn-icon" onClick={loadResources} disabled={resourcesLoading}>
              <Icon name="refresh" size={16} /> Refresh
            </button>
            <Link className="btn btn-icon" to="/clusters?new=1">
              <Icon name="plus" size={16} /> Create cluster
            </Link>
          </>
        }
      />

      <section className="stat-grid">
        <StatCard
          label="Total clusters"
          value={dashNum ? '—' : clusters.length}
          hint={dashNum ? undefined : `${ready} healthy`}
          hintTone="ok"
        />
        <StatCard
          label="Machines"
          value={dashNum ? '—' : totalNodes}
          hint={
            dashNum
              ? undefined
              : `${cps + wks - provisioning} ready${provisioning ? ` · ${provisioning} provisioning` : ''}`
          }
        />
        <StatCard
          label="Providers"
          value={dashNum ? '—' : providers.length}
          hint={dashNum ? undefined : `${providersOnline} online`}
        />
        <StatCard
          label="Attention needed"
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

      <section className="dash-section">
        <div className="section-toolbar">
          <div>
            <h2 className="section-kicker" style={{ textTransform: 'none', letterSpacing: '-0.01em', fontSize: '0.875rem' }}>
              Cluster resource usage
            </h2>
            <p className="muted dash-section-sub" style={{ margin: '0.25rem 0 0' }}>
              Online clusters · live used / total
            </p>
          </div>
          <Link to="/clusters" className="section-link">
            View all clusters
          </Link>
        </div>
        {resourcesErr && <div className="error">{resourcesErr}</div>}
        {listLoading && clusters.length === 0 ? (
          <div className="dash-resource-grid">
            {[0, 1].map((i) => (
              <div key={i} className="live-resource-card cluster-resource-skeleton" aria-hidden>
                <div className="skeleton-line w-40" />
                <div className="skeleton-line w-80" />
              </div>
            ))}
          </div>
        ) : clusters.length === 0 ? (
          <div className="card dash-empty">
            <p className="muted" style={{ margin: 0 }}>
              No clusters yet. Add a provider, then create control planes and workers.
            </p>
            <div className="dash-empty-actions">
              <Link className="btn btn-icon" to="/providers">
                <Icon name="providers" size={16} /> Providers
              </Link>
              <Link className="btn btn-icon" to="/clusters?new=1">
                <Icon name="plus" size={16} /> Create cluster
              </Link>
            </div>
          </div>
        ) : displayResources.length === 0 ? (
          <div className="card dash-empty">
            <p className="muted" style={{ margin: 0 }}>
              No online clusters right now. Offline clusters still appear under All clusters.
            </p>
            <div className="dash-empty-actions">
              <Link className="btn btn-icon" to="/clusters">
                <Icon name="clusters" size={16} /> All clusters
              </Link>
            </div>
          </div>
        ) : (
          <div className="dash-resource-grid">
            {displayResources.map((s) => (
              <LiveClusterResourceCard
                key={s.cluster_id}
                summary={s}
                onOpen={() => nav(`/clusters/${s.cluster_id}`)}
              />
            ))}
          </div>
        )}
      </section>

      {(recent.length > 0 || !dashNum) && (
        <div className="dash-bottom-grid">
          <section className="dash-panel-card">
            <div className="section-toolbar">
              <div>
                <h2>Recent clusters</h2>
                <p className="panel-sub">Latest activity across your infrastructure</p>
              </div>
              <Link to="/clusters" className="section-link">
                View all clusters
              </Link>
            </div>
            {recent.length === 0 ? (
              <p className="muted" style={{ marginTop: '1.25rem' }}>No clusters yet.</p>
            ) : (
              <div className="dash-recent-list">
                {recent.map((c) => {
                  const tone = statusTone(c.status)
                  const nodes = (c.controlplanes || 0) + (c.workers || 0)
                  const ver = formatK8sVersion(c.k8s_version) || c.os_version || '—'
                  return (
                    <div
                      key={c.id}
                      className="dash-recent-row"
                      role="link"
                      tabIndex={0}
                      onClick={() => nav(`/clusters/${c.id}`)}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter' || e.key === ' ') {
                          e.preventDefault()
                          nav(`/clusters/${c.id}`)
                        }
                      }}
                    >
                      <div className="dash-recent-left">
                        <span className={`dash-recent-dot ${tone}`} aria-hidden />
                        <div style={{ minWidth: 0 }}>
                          <div className="dash-recent-name">{c.name}</div>
                          <div className="dash-recent-meta">
                            {nodes} machines · {c.status || 'unknown'}
                          </div>
                        </div>
                      </div>
                      <span className="dash-recent-ver">{ver}</span>
                    </div>
                  )
                })}
              </div>
            )}
          </section>

          <section className="dash-panel-card">
            <h2>System status</h2>
            <p className="panel-sub">Derived from live control-plane state</p>
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
      )}

      <section className="dash-section">
        <div className="section-toolbar">
          <h2 className="section-kicker" style={{ textTransform: 'none', letterSpacing: '-0.01em', fontSize: '0.875rem' }}>
            Providers
          </h2>
          <Link to="/providers" className="section-link">
            All providers
          </Link>
        </div>
        {providers.length === 0 ? (
          <div className="card dash-empty">
            <p className="muted" style={{ margin: 0 }}>
              No providers yet. Add Proxmox, vSphere, or Nutanix to create clusters.
            </p>
            <div className="dash-empty-actions">
              <Link className="btn btn-icon" to="/providers">
                <Icon name="plus" size={16} /> Add provider
              </Link>
            </div>
          </div>
        ) : displayProviders.length === 0 ? (
          <div className="card dash-empty">
            <p className="muted" style={{ margin: 0 }}>
              No online providers right now. Offline providers still appear under All providers.
            </p>
            <div className="dash-empty-actions">
              <Link className="btn btn-icon" to="/providers">
                <Icon name="providers" size={16} /> All providers
              </Link>
            </div>
          </div>
        ) : (
          <div className="dash-resource-grid">
            {displayProviders.map((s) => (
              <ProviderResourceCard
                key={s.provider_id}
                summary={s}
                onOpen={() => nav(`/providers/${s.provider_id}`)}
              />
            ))}
          </div>
        )}
        {providers.length > 0 && (
          <p className="muted dash-section-sub">
            {providersOnline} of {providers.length} hypervisors online
          </p>
        )}
      </section>
    </div>
  )
}
