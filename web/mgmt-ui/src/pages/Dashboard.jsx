import { useCallback, useEffect, useMemo, useState } from 'react'
import { Link, useNavigate } from 'react-router-dom'
import { api } from '../api'
import { Icon } from '../components/Icons'
import { ClusterStatusBadges } from '../components/ClusterStatusBadges'
import { ClusterMetaBadges, formatProviderKind, normalizeProviderKind } from '../components/ClusterMetaBadges'
import ResourceGauge, { GAUGE_BASE } from '../components/ResourceGauge'
import { ProviderStatusBadge } from '../components/ProviderStatusBadge'
import { useMgmtRefresh } from '../hooks/useMgmtEvents'
import { readSessionJson, writeSessionJson } from '../utils/sessionCache'

const BUSY = new Set(['deleting', 'provisioning', 'pending', 'upgrading'])
const RESOURCES_POLL_MS = 15000
const BUSY_FALLBACK_MS = 8000
const LIST_POLL_MS = 15000
const CACHE_CLUSTERS = 'pertisk_dash_clusters'
const CACHE_PROVIDERS = 'pertisk_dash_providers'
const CACHE_RESOURCES = 'pertisk_dash_resources'
const CACHE_PROVIDER_RES = 'pertisk_dash_provider_res'

function formatK8sVersion(v) {
  if (!v) return null
  const s = String(v).trim()
  if (!s) return null
  return s.startsWith('v') ? s : `v${s}`
}

function ClusterResourceCard({ summary, onOpen }) {
  const version = formatK8sVersion(summary.k8s_version)
  const nodes = summary.node_count
  const statusClass = summary.status || 'unknown'
  const avail = summary.availability || 'unknown'
  const cardClass = [
    'card',
    'cluster-resource-card',
    'cluster-resource-card-large',
    'tone-cluster',
    `status-${statusClass}`,
    statusClass === 'ready' ? `avail-${avail}` : '',
  ]
    .filter(Boolean)
    .join(' ')

  return (
    <article
      className={cardClass}
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
      <div className="cluster-resource-head">
        <div className="cluster-resource-identity">
          <div className="cluster-resource-icon" aria-hidden>
            <Icon name="clusters" size={18} />
          </div>
          <div className="cluster-resource-title">
            <h3 className="cluster-resource-name">{summary.cluster_name}</h3>
            <div className="cluster-resource-tags">
              {version && <span className="cluster-resource-chip mono-inline">{version}</span>}
              <span className="cluster-resource-chip">
                {nodes} node{nodes === 1 ? '' : 's'}
              </span>
              {(summary.arch || summary.provider_kind) && (
                <ClusterMetaBadges arch={summary.arch} providerKind={summary.provider_kind} />
              )}
            </div>
          </div>
        </div>
        <ClusterStatusBadges status={summary.status} availability={summary.availability} />
      </div>
      <div className="resource-gauge-row">
        <ResourceGauge label="CPU" icon="cpu" metric={summary.cpu} color={GAUGE_BASE.cpu} size="lg" />
        <ResourceGauge label="Memory" icon="memory" metric={summary.memory} color={GAUGE_BASE.memory} size="lg" />
        <ResourceGauge label="Disk" icon="disk" metric={summary.disk} color={GAUGE_BASE.disk} size="lg" />
      </div>
      {summary.error && summary.status === 'ready' && (
        <p className="muted cluster-resource-soft-err" title={summary.error}>
          <Icon name="alert" size={12} />
          {summary.error}
        </p>
      )}
    </article>
  )
}

function ProviderResourceCard({ summary, onOpen }) {
  const kind = normalizeProviderKind(summary.kind)
  const avail = summary.availability || 'unknown'
  const cardClass = [
    'card',
    'cluster-resource-card',
    'cluster-resource-card-compact',
    'tone-provider',
    avail === 'online' ? 'status-ready avail-online' : '',
    avail === 'offline' ? 'status-ready avail-offline' : '',
  ]
    .filter(Boolean)
    .join(' ')

  return (
    <article
      className={cardClass}
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
      <div className="cluster-resource-head">
        <div className="cluster-resource-identity">
          <div className="cluster-resource-icon" aria-hidden>
            <Icon name="providers" size={18} />
          </div>
          <div className="cluster-resource-title">
            <h3 className="cluster-resource-name">{summary.provider_name}</h3>
            <div className="cluster-resource-tags">
              <span className={`badge kind kind-${kind}`}>{formatProviderKind(summary.kind)}</span>
              {summary.node && <span className="cluster-resource-chip">{summary.node}</span>}
              {summary.storage && (
                <span className="cluster-resource-chip mono-inline">{summary.storage}</span>
              )}
            </div>
          </div>
        </div>
        <ProviderStatusBadge availability={avail} showUnknown />
      </div>
      <div className="resource-gauge-row">
        <ResourceGauge label="CPU" icon="cpu" metric={summary.cpu} color={GAUGE_BASE.cpu} size="sm" />
        <ResourceGauge label="Memory" icon="memory" metric={summary.memory} color={GAUGE_BASE.memory} size="sm" />
        <ResourceGauge label="Disk" icon="disk" metric={summary.disk} color={GAUGE_BASE.disk} size="sm" />
      </div>
      {summary.error && (
        <p className="muted cluster-resource-soft-err" title={summary.error}>
          <Icon name="alert" size={12} />
          {summary.error}
        </p>
      )}
    </article>
  )
}

function placeholderSummary(c) {
  const nodes = (c.controlplanes || 0) + (c.workers || 0)
  const empty = { used: null, total: null, percent: null, unit: '', display_used: null, display_total: null, error: null }
  return {
    cluster_id: c.id,
    cluster_name: c.name,
    status: c.status || 'unknown',
    availability: c.availability || 'unknown',
    k8s_version: c.k8s_version || '',
    node_count: nodes,
    provider_kind: c.provider_kind,
    provider_name: c.provider_name,
    arch: c.arch,
    cpu: { ...empty, unit: 'cores' },
    memory: { ...empty, unit: 'GiB' },
    disk: { ...empty, unit: 'GiB' },
    error: null,
    _placeholder: true,
  }
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
        // Keep prior resources; only surface the error.
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

  useMgmtRefresh(load)

  useEffect(() => {
    const busy = clusters.some((c) => BUSY.has(c.status))
    if (!busy) return undefined
    const t = setInterval(load, BUSY_FALLBACK_MS)
    return () => clearInterval(t)
  }, [clusters, load])

  useEffect(() => {
    if (clusters.length === 0) return undefined
    const busy = clusters.some((c) => BUSY.has(c.status))
    if (busy) return undefined
    const t = setInterval(load, LIST_POLL_MS)
    return () => clearInterval(t)
  }, [clusters, load])

  const awaitingLiveMetrics = resources.length === 0
    || resources.some((r) => r.cpu?.percent == null && r.memory?.percent == null && r.status === 'ready' && r.availability !== 'offline')

  useEffect(() => {
    if (clusters.length === 0) return undefined
    let n = 0
    let timer
    const tick = () => {
      n += 1
      loadResources()
      timer = setTimeout(tick, n < 4 && awaitingLiveMetrics ? 2500 : RESOURCES_POLL_MS)
    }
    timer = setTimeout(tick, awaitingLiveMetrics ? 2500 : RESOURCES_POLL_MS)
    return () => clearTimeout(timer)
  }, [clusters.length, awaitingLiveMetrics, loadResources])

  useEffect(() => {
    if (providers.length === 0) return undefined
    const t = setInterval(loadProviderResources, RESOURCES_POLL_MS)
    return () => clearInterval(t)
  }, [providers.length, loadProviderResources])

  const displayResources = useMemo(() => {
    if (clusters.length === 0) return resources
    const byId = new Map(resources.map((r) => [r.cluster_id, r]))
    return clusters.map((c) => {
      const live = byId.get(c.id) || placeholderSummary(c)
      return {
        ...live,
        provider_kind: c.provider_kind,
        provider_name: c.provider_name,
        arch: c.arch,
      }
    })
  }, [resources, clusters])

  const ready = clusters.filter((c) => c.status === 'ready').length
  const cps = clusters.reduce((n, c) => n + (c.controlplanes || 0), 0)
  const wks = clusters.reduce((n, c) => n + (c.workers || 0), 0)
  const providersOnline = providers.filter((p) => p.availability === 'online').length
  const providersOffline = providers.filter((p) => p.availability === 'offline').length
  const recent = clusters.slice(0, 8)
  const dashNum = listLoading && clusters.length === 0
  const provNum = listLoading && providers.length === 0

  const displayProviders = useMemo(() => {
    if (providers.length === 0) return providerRes
    const byId = new Map(providerRes.map((r) => [r.provider_id, r]))
    return providers.map((p) => byId.get(p.id) || placeholderProvider(p))
  }, [providerRes, providers])

  return (
    <div className="dash-page">
      <div className="page-head">
        <div>
          <h1><Icon name="dashboard" size={22} /> Dashboard</h1>
          <p className="muted dash-lead">Clusters and hypervisors at a glance</p>
        </div>
        <Link className="btn btn-icon" to="/clusters?new=1">
          <Icon name="plus" size={16} /> Create cluster
        </Link>
      </div>

      <div className="dash-stats">
        <div className="dash-stat-group dash-stat-group-clusters">
          <div className="dash-stat-kicker">
            <Icon name="clusters" size={14} /> Clusters
          </div>
          <div className="dash-stat-row">
            <div className="stat">
              <div className="label">Total</div>
              <div className="value">{dashNum ? '—' : clusters.length}</div>
            </div>
            <div className="stat">
              <div className="label">Ready</div>
              <div className="value">{dashNum ? '—' : ready}</div>
            </div>
            <div className="stat">
              <div className="label">Control planes</div>
              <div className="value">{dashNum ? '—' : cps}</div>
            </div>
            <div className="stat">
              <div className="label">Workers</div>
              <div className="value">{dashNum ? '—' : wks}</div>
            </div>
          </div>
        </div>
        <div className="dash-stat-group dash-stat-group-providers">
          <div className="dash-stat-kicker">
            <Icon name="providers" size={14} /> Providers
          </div>
          <div className="dash-stat-row">
            <div className="stat">
              <div className="label">Total</div>
              <div className="value">{provNum ? '—' : providers.length}</div>
            </div>
            <div className="stat">
              <div className="label">Online</div>
              <div className="value">{provNum ? '—' : providersOnline}</div>
            </div>
            <div className="stat">
              <div className="label">Offline</div>
              <div className="value">{provNum ? '—' : providersOffline}</div>
            </div>
          </div>
        </div>
      </div>

      <div className="dash-boards">
        <section className="dash-panel dash-panel-clusters">
          <div className="section-head dash-resources-head">
            <div>
              <h2 className="card-title">
                <Icon name="clusters" size={18} /> Clusters
              </h2>
              <p className="muted dash-section-sub">
                Live CPU, memory, and disk · updates every {RESOURCES_POLL_MS / 1000}s
                {resourcesLoading && (resources.length === 0 || awaitingLiveMetrics) ? ' · loading metrics…' : ''}
              </p>
            </div>
            <div className="dash-resources-actions">
              <Link className="secondary btn-icon" to="/clusters">
                <Icon name="clusters" size={14} /> All clusters
              </Link>
              <button type="button" className="secondary btn-icon" onClick={loadResources} disabled={resourcesLoading}>
                <Icon name="refresh" size={14} /> Refresh
              </button>
            </div>
          </div>

          {resourcesErr && <div className="error">{resourcesErr}</div>}

          {listLoading && clusters.length === 0 ? (
            <div className="cluster-resource-grid dash-cluster-grid">
              {[0, 1].map((i) => (
                <div key={i} className="card cluster-resource-card cluster-resource-card-large tone-cluster cluster-resource-skeleton" aria-hidden>
                  <div className="skeleton-line w-40" />
                  <div className="skeleton-line w-80" />
                  <div className="resource-gauge-row">
                    <div className="skeleton-line" />
                    <div className="skeleton-line" />
                    <div className="skeleton-line" />
                  </div>
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
          ) : (
            <div className="cluster-resource-grid dash-cluster-grid">
              {displayResources.map((s) => (
                <ClusterResourceCard
                  key={s.cluster_id}
                  summary={s}
                  onOpen={() => nav(`/clusters/${s.cluster_id}`)}
                />
              ))}
            </div>
          )}
        </section>

        <section className="dash-panel dash-panel-providers">
          <div className="section-head dash-resources-head">
            <div>
              <h2 className="card-title">
                <Icon name="providers" size={18} /> Providers
              </h2>
              <p className="muted dash-section-sub">
                Hypervisor CPU, memory, and disk (used / available / total)
              </p>
            </div>
            <div className="dash-resources-actions">
              <Link className="secondary btn-icon" to="/providers">
                <Icon name="providers" size={14} /> All providers
              </Link>
              <button type="button" className="secondary btn-icon" onClick={loadProviderResources}>
                <Icon name="refresh" size={14} /> Refresh
              </button>
            </div>
          </div>
          {displayProviders.length === 0 ? (
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
          ) : (
            <div className="cluster-resource-grid dash-provider-grid">
              {displayProviders.map((s) => (
                <ProviderResourceCard
                  key={s.provider_id}
                  summary={s}
                  onOpen={() => nav(`/providers/${s.provider_id}`)}
                />
              ))}
            </div>
          )}
        </section>
      </div>

      {recent.length > 0 && (
        <div className="card dash-recent">
          <div className="section-head dash-recent-head">
            <h2 className="card-title">
              <Icon name="clusters" size={18} /> Recent
            </h2>
            <Link className="linkish" to="/clusters">View all</Link>
          </div>
          <table>
            <thead>
              <tr><th>Name</th><th>Status</th><th>Arch / Provider</th><th>Topology</th></tr>
            </thead>
            <tbody>
              {recent.map((c) => {
                const to = `/clusters/${c.id}`
                return (
                  <tr
                    key={c.id}
                    className="row-click"
                    tabIndex={0}
                    role="link"
                    onClick={() => nav(to)}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter' || e.key === ' ') {
                        e.preventDefault()
                        nav(to)
                      }
                    }}
                  >
                    <td><span className="row-click-label">{c.name}</span></td>
                    <td>
                      <ClusterStatusBadges status={c.status} availability={c.availability} />
                    </td>
                    <td>
                      <ClusterMetaBadges arch={c.arch} providerKind={c.provider_kind} />
                    </td>
                    <td>{c.controlplanes} CP / {c.workers} WK{c.vip ? ` · VIP ${c.vip}` : ''}</td>
                  </tr>
                )
              })}
            </tbody>
          </table>
        </div>
      )}
    </div>
  )
}
