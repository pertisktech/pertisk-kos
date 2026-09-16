import { useCallback, useEffect, useMemo, useState } from 'react'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { api } from '../api'
import { Icon } from '../components/Icons'
import PageHeader from '../components/PageHeader'
import StatCard from '../components/StatCard'
import ClusterCard, { placeholderSummary } from '../components/ClusterCard'
import ClusterWizard from '../components/ClusterWizard'
import { useMgmtRefresh } from '../hooks/useMgmtEvents'
import { readSessionJson, writeSessionJson } from '../utils/sessionCache'

const CACHE_CLUSTERS = 'pertisk_dash_clusters'

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
      }
    })
  }, [list, metrics])

  const ready = list.filter((c) => c.status === 'ready').length
  const attention = list.filter((c) => c.status === 'error' || c.status === 'degraded' || c.status === 'failed').length

  return (
    <div className="dash-page">
      <PageHeader
        title="Clusters"
        description="Every Kubernetes cluster reconciled by the control plane."
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
        <StatCard label="Total clusters" value={loaded ? list.length : '—'} icon="clusters" />
        <StatCard label="Ready" value={loaded ? ready : '—'} icon="check" />
        <StatCard label="Needs attention" value={loaded ? attention : '—'} icon="alert" />
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
        <section className="cluster-card-grid">
          {cards.map((s) => (
            <ClusterCard
              key={s.cluster_id}
              summary={s}
              onOpen={() => nav(`/clusters/${s.cluster_id}`)}
            />
          ))}
        </section>
      )}

      <ClusterWizard open={wizardOpen} onClose={() => setWizardOpen(false)} />
    </div>
  )
}
