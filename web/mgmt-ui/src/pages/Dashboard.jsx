import { useCallback, useEffect, useMemo, useState } from 'react'
import { Link, useNavigate } from 'react-router-dom'
import { Cell, Pie, PieChart, ResponsiveContainer } from 'recharts'
import { api } from '../api'
import { Icon } from '../components/Icons'

const BUSY = new Set(['deleting', 'provisioning', 'pending', 'upgrading'])
const RESOURCES_POLL_MS = 15000

const GAUGE_COLORS = {
  cpu: 'var(--accent)',
  memory: 'var(--success)',
  disk: '#60a5fa',
  track: 'color-mix(in srgb, var(--border) 70%, transparent)',
}

function gaugeData(percent) {
  const p = typeof percent === 'number' && Number.isFinite(percent) ? Math.min(100, Math.max(0, percent)) : 0
  return [
    { name: 'used', value: p },
    { name: 'free', value: Math.max(0, 100 - p) },
  ]
}

function ResourceGauge({ label, metric, color }) {
  const pct = metric?.percent
  const hasPct = typeof pct === 'number' && Number.isFinite(pct)
  const data = useMemo(() => gaugeData(hasPct ? pct : 0), [hasPct, pct])
  const usedLabel = metric?.display_used || (metric?.used != null ? String(metric.used) : '—')
  const totalLabel = metric?.display_total || (metric?.total != null ? String(metric.total) : '—')

  return (
    <div className="resource-gauge">
      <div className="resource-gauge-label">{label}</div>
      <div className="resource-gauge-chart">
        <ResponsiveContainer width="100%" height={100}>
          <PieChart>
            <Pie
              data={data}
              dataKey="value"
              cx="50%"
              cy="50%"
              startAngle={90}
              endAngle={-270}
              innerRadius={30}
              outerRadius={40}
              stroke="none"
              isAnimationActive={false}
            >
              <Cell fill={hasPct ? color : GAUGE_COLORS.track} />
              <Cell fill={GAUGE_COLORS.track} />
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
  return (
    <article
      className="card cluster-resource-card"
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
        <div className="cluster-resource-title">
          <h3 className="cluster-resource-name">{summary.cluster_name}</h3>
          {version && <span className="cluster-resource-version mono-inline">{version}</span>}
        </div>
        <div className="cluster-resource-head-meta">
          <span className={`badge ${summary.status}`}>{summary.status}</span>
          <span className="muted cluster-resource-nodes">
            {summary.node_count} node{summary.node_count === 1 ? '' : 's'}
          </span>
        </div>
      </div>
      <div className="resource-gauge-row">
        <ResourceGauge label="CPU" metric={summary.cpu} color={GAUGE_COLORS.cpu} />
        <ResourceGauge label="Memory" metric={summary.memory} color={GAUGE_COLORS.memory} />
        <ResourceGauge label="Disk" metric={summary.disk} color={GAUGE_COLORS.disk} />
      </div>
      {summary.error && summary.status === 'ready' && (
        <p className="muted cluster-resource-soft-err" title={summary.error}>{summary.error}</p>
      )}
    </article>
  )
}

export default function Dashboard() {
  const nav = useNavigate()
  const [clusters, setClusters] = useState([])
  const [providers, setProviders] = useState([])
  const [health, setHealth] = useState(null)
  const [resources, setResources] = useState([])
  const [resourcesErr, setResourcesErr] = useState('')

  const load = useCallback(() => {
    Promise.all([
      api('/clusters').catch(() => []),
      api('/providers').catch(() => []),
      fetch('/api/health').then((r) => r.json()).catch(() => null),
    ]).then(([c, p, h]) => {
      setClusters(c)
      setProviders(p)
      setHealth(h)
    })
  }, [])

  const loadResources = useCallback(() => {
    api('/dashboard/resources')
      .then((rows) => {
        setResources(Array.isArray(rows) ? rows : [])
        setResourcesErr('')
      })
      .catch((e) => {
        setResourcesErr(e.message || 'failed to load resources')
      })
  }, [])

  useEffect(() => {
    load()
    loadResources()
  }, [load, loadResources])

  useEffect(() => {
    const busy = clusters.some((c) => BUSY.has(c.status))
    if (!busy) return undefined
    const t = setInterval(load, 2000)
    return () => clearInterval(t)
  }, [clusters, load])

  useEffect(() => {
    if (clusters.length === 0) return undefined
    const t = setInterval(loadResources, RESOURCES_POLL_MS)
    return () => clearInterval(t)
  }, [clusters.length, loadResources])

  const ready = clusters.filter((c) => c.status === 'ready').length
  const cps = clusters.reduce((n, c) => n + (c.controlplanes || 0), 0)
  const wks = clusters.reduce((n, c) => n + (c.workers || 0), 0)

  return (
    <div>
      <div className="page-head">
        <h1><Icon name="dashboard" size={22} /> Dashboard</h1>
        <Link className="btn btn-icon" to="/clusters/new">
          <Icon name="plus" size={16} /> Create cluster
        </Link>
      </div>
      <div className="grid-stats">
        <div className="stat"><div className="label">Clusters</div><div className="value">{clusters.length}</div></div>
        <div className="stat"><div className="label">Ready</div><div className="value">{ready}</div></div>
        <div className="stat"><div className="label">Control planes</div><div className="value">{cps}</div></div>
        <div className="stat"><div className="label">Workers</div><div className="value">{wks}</div></div>
        <div className="stat"><div className="label">Providers</div><div className="value">{providers.length}</div></div>
      </div>
      <div className="card">
        <h2 className="card-title"><Icon name="play" size={18} /> API</h2>
        <p className="muted">{health ? `status: ${health.status}` : 'unreachable'}</p>
      </div>

      <div className="section-head dash-resources-head">
        <div>
          <h2 className="card-title" style={{ marginBottom: 0 }}>
            <Icon name="cpu" size={18} /> Cluster resources
          </h2>
          <p className="muted">
            CPU / memory from <code className="mono-inline">kubectl top</code>
            {' '}· disk from node filesystem stats (poll {RESOURCES_POLL_MS / 1000}s)
          </p>
        </div>
        <button type="button" className="secondary btn-icon" onClick={loadResources}>
          <Icon name="play" size={14} /> Refresh
        </button>
      </div>

      {resourcesErr && <div className="error">{resourcesErr}</div>}

      {clusters.length === 0 ? (
        <div className="card">
          <p className="muted">No clusters yet. Configure a Proxmox provider, then create M CP + N workers.</p>
        </div>
      ) : resources.length === 0 && !resourcesErr ? (
        <div className="card"><p className="muted">Loading resource summaries…</p></div>
      ) : (
        <div className="cluster-resource-grid">
          {resources.map((s) => (
            <ClusterResourceCard
              key={s.cluster_id}
              summary={s}
              onOpen={() => nav(`/clusters/${s.cluster_id}`)}
            />
          ))}
        </div>
      )}

      <div className="card">
        <h2 className="card-title"><Icon name="clusters" size={18} /> Recent clusters</h2>
        {clusters.length === 0 ? (
          <p className="muted">No clusters yet.</p>
        ) : (
          <table>
            <thead>
              <tr><th>Name</th><th>Status</th><th>Topology</th></tr>
            </thead>
            <tbody>
              {clusters.slice(0, 8).map((c) => {
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
                    <td><span className={`badge ${c.status}`}>{c.status}</span></td>
                    <td>{c.controlplanes} CP / {c.workers} WK{c.vip ? ` · VIP ${c.vip}` : ''}</td>
                  </tr>
                )
              })}
            </tbody>
          </table>
        )}
      </div>
    </div>
  )
}
