import { useCallback, useEffect, useMemo, useState } from 'react'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { api } from '../api'
import { Icon } from '../components/Icons'
import PageHeader from '../components/PageHeader'
import StatCard from '../components/StatCard'
import { placeholderSummary, formatK8sVersion } from '../components/ClusterCard'
import ClusterWizard from '../components/ClusterWizard'
import { useMgmtRefresh } from '../hooks/useMgmtEvents'
import { readSessionJson, writeSessionJson } from '../utils/sessionCache'

const CACHE_CLUSTERS = 'pertisk_dash_clusters'

function formatMetric(m) {
  if (!m) return '—'
  const used = m.display_used ?? m.used
  const total = m.display_total ?? m.total
  if (used == null && total == null) return '—'
  const unit = m.unit ? ` ${m.unit}` : ''
  if (total == null) return `${used}${unit}`
  if (used == null) return `— / ${total}${unit}`
  return `${used} / ${total}${unit}`
}

function statusTone(status) {
  if (status === 'ready') return 'ok'
  if (status === 'error' || status === 'failed' || status === 'degraded') return 'err'
  return 'warn'
}

function statusLabel(status) {
  if (status === 'ready') return 'Healthy'
  if (!status) return 'Unknown'
  return status.charAt(0).toUpperCase() + status.slice(1)
}

function CompactClusterRow({ summary, onOpen }) {
  const tone = statusTone(summary.status)
  const cps = Number(summary.controlplanes) || 0
  const wks = Number(summary.workers) || 0
  const nodes = Number(summary.node_count) || cps + wks
  const readyHint = summary.status === 'ready' ? 'machines ready' : 'machines'
  const desc =
    summary.provider_name ||
    [formatK8sVersion(summary.k8s_version), summary.vip ? `VIP ${summary.vip}` : null]
      .filter(Boolean)
      .join(' · ') ||
    'Managed cluster'

  return (
    <button type="button" className="compact-cluster-row" onClick={onOpen}>
      <div className="compact-cluster-identity">
        <span className={`compact-cluster-icon ${tone}`} aria-hidden>
          <Icon name="clusters" size={16} />
        </span>
        <div style={{ minWidth: 0 }}>
          <div className="compact-cluster-name">{summary.cluster_name}</div>
          <div className="compact-cluster-desc">{desc}</div>
        </div>
      </div>
      <div>
        <span className="compact-cluster-mobile-label">Status</span>
        <span className={`compact-cluster-status ${tone}`}>
          <span className="dot" aria-hidden />
          {statusLabel(summary.status)}
        </span>
      </div>
      <div>
        <span className="compact-cluster-mobile-label">Machines</span>
        <div className="compact-cluster-cell">{nodes}</div>
        <div className="compact-cluster-cell-sub">
          {cps} CP · {wks} WK · {readyHint}
        </div>
      </div>
      <div>
        <span className="compact-cluster-mobile-label">CPU</span>
        <div className="compact-cluster-cell">{formatMetric(summary.cpu)}</div>
        <div className="compact-cluster-cell-sub">CPU used / total</div>
      </div>
      <div>
        <span className="compact-cluster-mobile-label">Memory</span>
        <div className="compact-cluster-cell">{formatMetric(summary.memory)}</div>
        <div className="compact-cluster-cell-sub">memory used / total</div>
      </div>
    </button>
  )
}

export default function Clusters() {
  const nav = useNavigate()
  const [list, setList] = useState(() => readSessionJson(CACHE_CLUSTERS, []))
  const [metrics, setMetrics] = useState({})
  const [loaded, setLoaded] = useState(() => Array.isArray(readSessionJson(CACHE_CLUSTERS, null)))
  const [error, setError] = useState('')
  const [search, setSearch] = useSearchParams()
  const expectDelete = search.get('deleting')
  const [wizardOpen, setWizardOpen] = useState(search.get('new') === '1')

  useEffect(() => {
    if (search.get('new') === '1') {
      setWizardOpen(true)
      const next = new URLSearchParams(search)
      next.delete('new')
      setSearch(next, { replace: true })
    }
  }, [search, setSearch])

  const load = useCallback(() => {
    Promise.all([
      api('/clusters'),
      api('/dashboard/resources').catch(() => []),
    ])
      .then(([rows, res]) => {
        const next = Array.isArray(rows) ? rows : []
        setList(next)
        writeSessionJson(CACHE_CLUSTERS, next)
        const map = {}
        for (const r of Array.isArray(res) ? res : []) {
          if (r?.cluster_id) map[r.cluster_id] = r
        }
        setMetrics(map)
        setLoaded(true)
        if (expectDelete && !next.some((c) => c.id === expectDelete)) {
          setSearch({}, { replace: true })
        }
      })
      .catch((e) => {
        setError(e.message)
        setLoaded(true)
      })
  }, [expectDelete, setSearch])

  useEffect(() => {
    load()
  }, [load])

  useMgmtRefresh(load)

  useEffect(() => {
    function onFocus() {
      load()
    }
    window.addEventListener('focus', onFocus)
    return () => window.removeEventListener('focus', onFocus)
  }, [load])

  const cards = useMemo(() => {
    const byId = new Map(Object.entries(metrics))
    return list.map((c) => {
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
  }, [list, metrics])

  const ready = list.filter((c) => c.status === 'ready').length
  const totalMachines = list.reduce(
    (n, c) => n + (c.controlplanes || 0) + (c.workers || 0),
    0,
  )

  return (
    <div className="dash-page">
      <PageHeader
        title="Clusters"
        description="Create and manage your Talos Kubernetes clusters."
        actions={
          <button type="button" className="btn btn-icon" onClick={() => setWizardOpen(true)}>
            <Icon name="plus" size={16} /> Create cluster
          </button>
        }
      />
      {error && <div className="error">{error}</div>}
      {expectDelete && (
        <p className="muted" style={{ margin: 0 }}>
          Deleting cluster… the list will update when the job finishes.
        </p>
      )}

      <section className="stat-grid stat-grid-3">
        <StatCard label="Total clusters" value={loaded ? list.length : '—'} />
        <StatCard label="Healthy" value={loaded ? ready : '—'} hintTone="ok" />
        <StatCard label="Machines" value={loaded ? totalMachines : '—'} />
      </section>

      {list.length === 0 ? (
        <div className="card dash-empty">
          <p className="muted" style={{ margin: 0 }}>
            {loaded
              ? 'No clusters. Create with M control planes (+ VIP if M>1) and N workers.'
              : 'Loading clusters…'}
          </p>
        </div>
      ) : (
        <section className="dash-section">
          <div className="section-toolbar">
            <div>
              <h2 className="section-kicker" style={{ textTransform: 'none', letterSpacing: '-0.01em', fontSize: '0.875rem' }}>
                All clusters
              </h2>
              <p className="muted dash-section-sub" style={{ margin: '0.25rem 0 0' }}>
                Your managed Kubernetes infrastructure
              </p>
            </div>
          </div>
          <div className="compact-cluster-table">
            <div className="compact-cluster-head">
              <span>Cluster</span>
              <span>Status</span>
              <span>Machines</span>
              <span>CPU</span>
              <span>Memory</span>
            </div>
            <div className="compact-cluster-body">
              {cards.map((s) => (
                <CompactClusterRow
                  key={s.cluster_id}
                  summary={s}
                  onOpen={() => nav(`/clusters/${s.cluster_id}`)}
                />
              ))}
            </div>
          </div>
        </section>
      )}

      <ClusterWizard open={wizardOpen} onClose={() => setWizardOpen(false)} />
    </div>
  )
}
