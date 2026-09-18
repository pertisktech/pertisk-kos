import { useCallback, useEffect, useMemo, useState } from 'react'
import { Link, useNavigate } from 'react-router-dom'
import { api } from '../api'
import { Icon } from '../components/Icons'
import PageHeader from '../components/PageHeader'
import StatCard from '../components/StatCard'
import ClusterCard, { placeholderSummary } from '../components/ClusterCard'
import ResourceGauge, { GAUGE_BASE } from '../components/ResourceGauge'
import { ClusterStatusBadges } from '../components/ClusterStatusBadges'
import { ClusterMetaBadges, formatProviderKind, normalizeProviderKind, providerKindGlyph } from '../components/ClusterMetaBadges'
import { ProviderStatusBadge } from '../components/ProviderStatusBadge'
import { useMgmtRefresh } from '../hooks/useMgmtEvents'
import { readSessionJson, writeSessionJson } from '../utils/sessionCache'

const CACHE_CLUSTERS = 'pertisk_dash_clusters'
const CACHE_PROVIDERS = 'pertisk_dash_providers'
const CACHE_RESOURCES = 'pertisk_dash_resources'
const CACHE_PROVIDER_RES = 'pertisk_dash_provider_res'

function ProviderResourceCard({ summary, onOpen }) {
  const kind = normalizeProviderKind(summary.kind)
  const avail = summary.availability || 'unknown'
  const cardClass = [
    'cluster-card',
    'provider-card',
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
      <div className="cluster-card-head">
        <div className="cluster-card-identity">
          <span className="cluster-card-icon entity-card-glyph" aria-hidden>
            {providerKindGlyph(summary.kind)}
          </span>
          <div className="cluster-card-title">
            <p className="cluster-card-name">{summary.provider_name}</p>
            <p className="cluster-card-meta">
              <span className={`badge kind kind-${kind}`}>{formatProviderKind(summary.kind)}</span>
              {summary.node ? <span>{summary.node}</span> : null}
            </p>
          </div>
        </div>
        <ProviderStatusBadge availability={avail} showUnknown />
      </div>
      <div className="cluster-card-body">
      {summary.storage ? (
        <div className="cluster-card-tags">
          <span className="tag tag-mono">{summary.storage}</span>
        </div>
      ) : null}
      <div className="cluster-card-meters">
        <ResourceGauge label="CPU" icon="cpu" metric={summary.cpu} color={GAUGE_BASE.cpu} layout="row" />
        <ResourceGauge label="Memory" icon="memory" metric={summary.memory} color={GAUGE_BASE.memory} layout="row" />
        <ResourceGauge label="Disk" icon="disk" metric={summary.disk} color={GAUGE_BASE.disk} layout="row" />
      </div>
      {summary.error && avail !== 'offline' && (
        <p className="muted cluster-resource-soft-err" title={summary.error}>
          <Icon name="alert" size={12} />
          {summary.error}
        </p>
      )}
      </div>
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
    if (clusters.length === 0) return resources
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
      }
    })
  }, [resources, clusters])

  const ready = clusters.filter((c) => c.status === 'ready').length
  const cps = clusters.reduce((n, c) => n + (c.controlplanes || 0), 0)
  const wks = clusters.reduce((n, c) => n + (c.workers || 0), 0)
  const totalNodes = cps + wks
  const providersOnline = providers.filter((p) => p.availability === 'online').length
  const recent = clusters.slice(0, 8)
  const dashNum = listLoading && clusters.length === 0
  const liveOs = clusters.find((c) => c.os_version)?.os_version || clusters.find((c) => c.k8s_version)?.k8s_version

  const displayProviders = useMemo(() => {
    if (providers.length === 0) return providerRes
    const byId = new Map(providerRes.map((r) => [r.provider_id, r]))
    return providers.map((p) => byId.get(p.id) || placeholderProvider(p))
  }, [providerRes, providers])

  return (
    <div className="dash-page">
      <PageHeader
        title="Dashboard"
        description="Clusters and hypervisors at a glance — live over WebSocket."
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
          label="Clusters"
          value={dashNum ? '—' : clusters.length}
          hint={`${dashNum ? '—' : ready} ready`}
          icon="clusters"
        />
        <StatCard
          label="Nodes"
          value={dashNum ? '—' : totalNodes}
          hint="across all providers"
          icon="machines"
        />
        <StatCard
          label="Control planes"
          value={dashNum ? '—' : cps}
          hint={`${dashNum ? '—' : wks} workers`}
          icon="shield"
        />
        <StatCard
          label="Node OS"
          value={dashNum ? '—' : liveOs ? (String(liveOs).startsWith('v') ? liveOs : `v${liveOs}`) : '—'}
          hint="latest seen on fleet"
          icon="packages"
        />
      </section>

      <section className="dash-section">
        <div className="section-toolbar">
          <h2 className="section-kicker">Live clusters</h2>
          <Link to="/clusters" className="section-link">
            All clusters
          </Link>
        </div>
        {resourcesErr && <div className="error">{resourcesErr}</div>}
        {listLoading && clusters.length === 0 ? (
          <div className="cluster-card-grid">
            {[0, 1].map((i) => (
              <div key={i} className="cluster-card cluster-resource-skeleton" aria-hidden>
                <div className="skeleton-line w-40" />
                <div className="skeleton-line w-80" />
                <div className="cluster-card-meters">
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
          <div className="cluster-card-grid">
            {displayResources.slice(0, 2).map((s) => (
              <ClusterCard
                key={s.cluster_id}
                summary={s}
                compact
                onOpen={() => nav(`/clusters/${s.cluster_id}`)}
              />
            ))}
          </div>
        )}
      </section>

      {recent.length > 0 && (
        <section className="dash-section">
          <h2 className="section-kicker">Recent</h2>
          <div className="table-shell">
            <table>
              <thead>
                <tr>
                  <th>Name</th>
                  <th>Status</th>
                  <th>Arch / Provider</th>
                  <th>Topology</th>
                  <th>Version</th>
                </tr>
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
                      <td className="muted">
                        {c.controlplanes} CP / {c.workers} WK{c.vip ? ` · VIP ${c.vip}` : ''}
                      </td>
                      <td className="mono-inline muted">{c.k8s_version || '—'}</td>
                    </tr>
                  )
                })}
              </tbody>
            </table>
          </div>
        </section>
      )}

      <section className="dash-section">
        <div className="section-toolbar">
          <h2 className="section-kicker">Providers</h2>
          <Link to="/providers" className="section-link">
            All providers
          </Link>
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
          <div className="cluster-card-grid cluster-card-grid-3">
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
