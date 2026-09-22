import { useCallback, useEffect, useMemo, useState } from 'react'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { api } from '../api'
import { Icon } from '../components/Icons'
import PageHeader from '../components/PageHeader'
import StatCard from '../components/StatCard'
import { ResourceBars } from '../components/ResourceBars'
import { placeholderSummary, formatK8sVersion } from '../components/ClusterCard'
import { formatProviderKind } from '../components/ClusterMetaBadges'
import ClusterWizard from '../components/ClusterWizard'
import { useMgmtRefresh } from '../hooks/useMgmtEvents'
import { readSessionJson, writeSessionJson } from '../utils/sessionCache'

const CACHE_CLUSTERS = 'pertisk_dash_clusters'

function statusTone(status) {
  if (status === 'ready') return 'ok'
  if (status === 'error' || status === 'failed' || status === 'degraded') return 'err'
  return 'warn'
}

function statusLabel(status) {
  if (status === 'ready') return 'healthy'
  if (!status) return 'unknown'
  return status
}

function FleetClusterRow({ summary, onOpen }) {
  const tone = statusTone(summary.status)
  const cps = Number(summary.controlplanes) || 0
  const wks = Number(summary.workers) || 0
  const nodes = Number(summary.node_count) || cps + wks
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
      <span className={`fleet-cell fleet-status ${tone}`}>{statusLabel(summary.status)}</span>
      <div className="fleet-cell fleet-cell-bars">
        <ResourceBars cpu={summary.cpu} memory={summary.memory} disk={summary.disk} />
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
  const [filter, setFilter] = useState('')
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
        k8s_version: c.k8s_version,
      }
    })
  }, [list, metrics])

  const filtered = useMemo(() => {
    const q = filter.trim().toLowerCase()
    if (!q) return cards
    return cards.filter((s) => {
      const hay = [s.cluster_name, s.provider_name, s.provider_kind, s.status, formatK8sVersion(s.k8s_version)]
        .filter(Boolean)
        .join(' ')
        .toLowerCase()
      return hay.includes(q)
    })
  }, [cards, filter])

  const ready = list.filter((c) => c.status === 'ready').length
  const totalMachines = list.reduce(
    (n, c) => n + (c.controlplanes || 0) + (c.workers || 0),
    0,
  )

  return (
    <div className="dash-page">
      <PageHeader
        title="Clusters"
        description="Create and manage your Kubernetes clusters."
        actions={
          <button type="button" className="btn btn-icon" onClick={() => setWizardOpen(true)}>
            <Icon name="plus" size={16} /> New cluster
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
        <StatCard icon="clusters" label="Clusters" value={loaded ? list.length : '—'} />
        <StatCard icon="check" label="Healthy" value={loaded ? ready : '—'} hintTone="ok" />
        <StatCard icon="machines" label="Nodes" value={loaded ? totalMachines : '—'} />
      </section>

      <section className="fleet-panel">
        <div className="fleet-panel-head">
          <div className="fleet-panel-title-row">
            <h2 className="fleet-panel-title">All clusters</h2>
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
        {list.length === 0 ? (
          <div className="fleet-empty">
            <p className="muted" style={{ margin: 0 }}>
              {loaded
                ? 'No clusters. Create with M control planes (+ VIP if M>1) and N workers.'
                : 'Loading clusters…'}
            </p>
          </div>
        ) : filtered.length === 0 ? (
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
                {filtered.map((s) => (
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

      <ClusterWizard open={wizardOpen} onClose={() => setWizardOpen(false)} />
    </div>
  )
}
