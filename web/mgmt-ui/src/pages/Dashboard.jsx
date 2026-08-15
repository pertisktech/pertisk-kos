import { useCallback, useEffect, useMemo, useState } from 'react'
import { Link, useNavigate } from 'react-router-dom'
import { Cell, Pie, PieChart, ResponsiveContainer } from 'recharts'
import { api } from '../api'
import { Icon } from '../components/Icons'
import { ClusterStatusBadges } from '../components/ClusterStatusBadges'
import { ClusterMetaBadges } from '../components/ClusterMetaBadges'
import { useMgmtRefresh } from '../hooks/useMgmtEvents'

const BUSY = new Set(['deleting', 'provisioning', 'pending', 'upgrading'])
const RESOURCES_POLL_MS = 15000
const BUSY_FALLBACK_MS = 8000

const GAUGE_BASE = {
  cpu: 'var(--accent)',
  memory: 'var(--success)',
  disk: 'var(--brand-deep)',
  track: 'color-mix(in srgb, var(--border) 70%, transparent)',
}

function gaugeFill(base, percent, hasPct) {
  if (!hasPct) return GAUGE_BASE.track
  if (percent >= 90) return 'var(--danger)'
  if (percent >= 75) return 'var(--warning)'
  return base
}

function gaugeData(percent) {
  const p = typeof percent === 'number' && Number.isFinite(percent) ? Math.min(100, Math.max(0, percent)) : 0
  return [
    { name: 'used', value: p },
    { name: 'free', value: Math.max(0, 100 - p) },
  ]
}

function ResourceGauge({ label, icon, metric, color }) {
  const pct = metric?.percent
  const hasPct = typeof pct === 'number' && Number.isFinite(pct)
  const data = useMemo(() => gaugeData(hasPct ? pct : 0), [hasPct, pct])
  const fill = gaugeFill(color, pct, hasPct)
  const usedLabel = metric?.display_used || (metric?.used != null ? String(metric.used) : '—')
  const totalLabel = metric?.display_total || (metric?.total != null ? String(metric.total) : '—')
  const level = !hasPct ? 'unknown' : pct >= 90 ? 'critical' : pct >= 75 ? 'warn' : 'ok'

  return (
    <div className={`resource-gauge resource-gauge-${level}`}>
      <div className="resource-gauge-label">
        {icon && <Icon name={icon} size={12} />}
        {label}
      </div>
      <div className="resource-gauge-chart">
        <ResponsiveContainer width="100%" height={88}>
          <PieChart>
            <Pie
              data={data}
              dataKey="value"
              cx="50%"
              cy="50%"
              startAngle={90}
              endAngle={-270}
              innerRadius={28}
              outerRadius={36}
              paddingAngle={hasPct && pct > 0 && pct < 100 ? 2 : 0}
              cornerRadius={4}
              stroke="none"
              isAnimationActive={false}
            >
              <Cell fill={fill} />
              <Cell fill={GAUGE_BASE.track} />
            </Pie>
          </PieChart>
        </ResponsiveContainer>
        <div className="resource-gauge-center">
          <span className="resource-gauge-pct">
            {hasPct ? `${Math.round(pct)}%` : '—'}
          </span>
        </div>
      </div>
      <div className="mono-inline resource-gauge-values">
        {usedLabel} <span className="muted">/</span> {totalLabel}
      </div>
      {metric?.error && (
        <div className="muted resource-gauge-err" title={metric.error}>{metric.error}</div>
      )}
    </div>
  )
}

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
            </div>
          </div>
        </div>
        <ClusterStatusBadges status={summary.status} availability={summary.availability} />
      </div>
      <div className="resource-gauge-row">
        <ResourceGauge label="CPU" icon="cpu" metric={summary.cpu} color={GAUGE_BASE.cpu} />
        <ResourceGauge label="Memory" icon="memory" metric={summary.memory} color={GAUGE_BASE.memory} />
        <ResourceGauge label="Disk" icon="disk" metric={summary.disk} color={GAUGE_BASE.disk} />
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
    cpu: { ...empty, unit: 'cores' },
    memory: { ...empty, unit: 'GiB' },
    disk: { ...empty, unit: 'GiB' },
    error: null,
    _placeholder: true,
  }
}

export default function Dashboard() {
  const nav = useNavigate()
  const [clusters, setClusters] = useState([])
  const [providers, setProviders] = useState([])
  const [resources, setResources] = useState([])
  const [resourcesErr, setResourcesErr] = useState('')
  const [resourcesLoading, setResourcesLoading] = useState(false)

  const load = useCallback(() => {
    Promise.all([
      api('/clusters').catch(() => []),
      api('/providers').catch(() => []),
    ]).then(([c, p]) => {
      setClusters(c)
      setProviders(p)
    })
  }, [])

  const loadResources = useCallback(() => {
    setResourcesLoading(true)
    api('/dashboard/resources')
      .then((rows) => {
        // Soft-fail: keep prior cards if the response is empty unexpectedly.
        if (Array.isArray(rows) && rows.length > 0) {
          setResources(rows)
        } else if (Array.isArray(rows)) {
          setResources(rows)
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

  useEffect(() => {
    load()
    loadResources()
  }, [load, loadResources])

  useMgmtRefresh(load)

  useEffect(() => {
    const busy = clusters.some((c) => BUSY.has(c.status))
    if (!busy) return undefined
    const t = setInterval(load, BUSY_FALLBACK_MS)
    return () => clearInterval(t)
  }, [clusters, load])

  useEffect(() => {
    if (clusters.length === 0) return undefined
    const t = setInterval(loadResources, RESOURCES_POLL_MS)
    return () => clearInterval(t)
  }, [clusters.length, loadResources])

  const displayResources = useMemo(() => {
    if (resources.length > 0) return resources
    // While live metrics load, show skeleton cards from the cluster list.
    if (clusters.length > 0) return clusters.map(placeholderSummary)
    return []
  }, [resources, clusters])

  const ready = clusters.filter((c) => c.status === 'ready').length
  const cps = clusters.reduce((n, c) => n + (c.controlplanes || 0), 0)
  const wks = clusters.reduce((n, c) => n + (c.workers || 0), 0)
  const providersOnline = providers.filter((p) => p.availability === 'online').length
  const providersOffline = providers.filter((p) => p.availability === 'offline').length
  const recent = clusters.slice(0, 8)

  return (
    <div className="dash-page">
      <div className="page-head">
        <h1><Icon name="dashboard" size={22} /> Dashboard</h1>
        <Link className="btn btn-icon" to="/clusters?new=1">
          <Icon name="plus" size={16} /> Create cluster
        </Link>
      </div>

      <div className="grid-stats">
        <div className="stat"><div className="label">Clusters</div><div className="value">{clusters.length}</div></div>
        <div className="stat"><div className="label">Ready</div><div className="value">{ready}</div></div>
        <div className="stat"><div className="label">Control planes</div><div className="value">{cps}</div></div>
        <div className="stat"><div className="label">Workers</div><div className="value">{wks}</div></div>
        <div className="stat">
          <div className="label">Providers</div>
          <div className="value">{providers.length}</div>
          {providers.length > 0 && (
            <div className="muted" style={{ marginTop: 4, display: 'flex', gap: 6, flexWrap: 'wrap' }}>
              {providersOnline > 0 && <span className="badge online">{providersOnline} online</span>}
              {providersOffline > 0 && <span className="badge offline">{providersOffline} offline</span>}
            </div>
          )}
        </div>
      </div>

      <div className="section-head dash-resources-head">
        <div>
          <h2 className="card-title" style={{ marginBottom: 0 }}>
            <Icon name="cpu" size={18} /> Clusters
          </h2>
          <p className="muted dash-section-sub">
            Live CPU, memory, and disk · updates every {RESOURCES_POLL_MS / 1000}s
            {resourcesLoading && resources.length === 0 ? ' · loading metrics…' : ''}
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

      {clusters.length === 0 ? (
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
        <div className="cluster-resource-grid">
          {displayResources.map((s) => (
            <ClusterResourceCard
              key={s.cluster_id}
              summary={s}
              onOpen={() => nav(`/clusters/${s.cluster_id}`)}
            />
          ))}
        </div>
      )}

      {recent.length > 0 && (
        <div className="card dash-recent">
          <div className="section-head" style={{ marginBottom: '0.75rem' }}>
            <h2 className="card-title" style={{ margin: 0 }}>
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
