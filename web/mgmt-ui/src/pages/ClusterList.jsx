import { useCallback, useEffect, useState } from 'react'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { api } from '../api'
import { Icon } from '../components/Icons'
import { ClusterStatusBadges } from '../components/ClusterStatusBadges'
import { formatProviderKind, normalizeProviderKind } from '../components/ClusterMetaBadges'
import { ProviderStatusBadge } from '../components/ProviderStatusBadge'
import ClusterWizard from '../components/ClusterWizard'
import UsageBar from '../components/UsageBar'
import { useMgmtRefresh } from '../hooks/useMgmtEvents'
import { readSessionJson, writeSessionJson } from '../utils/sessionCache'

const BUSY = new Set(['deleting', 'provisioning', 'pending', 'upgrading'])
const AVAIL_POLL_MS = 15000
/** Slow fallback while busy if SSE drops; primary updates come from events. */
const BUSY_FALLBACK_MS = 8000
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
        // Drop ?deleting= once that cluster is gone from the API.
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

  // Slow fallback while mid-job (SSE is primary).
  useEffect(() => {
    const busy = list.some((c) => BUSY.has(c.status)) || !!expectDelete
    if (!busy) return undefined
    const t = setInterval(load, BUSY_FALLBACK_MS)
    return () => clearInterval(t)
  }, [list, load, expectDelete])

  // Refresh online/offline even when idle.
  useEffect(() => {
    if (list.length === 0) return undefined
    const busy = list.some((c) => BUSY.has(c.status))
    if (busy) return undefined
    const t = setInterval(load, AVAIL_POLL_MS)
    return () => clearInterval(t)
  }, [list, load])

  useEffect(() => {
    function onFocus() {
      load()
    }
    window.addEventListener('focus', onFocus)
    return () => window.removeEventListener('focus', onFocus)
  }, [load])

  return (
    <div>
      <div className="page-head">
        <h1><Icon name="clusters" size={22} /> Clusters</h1>
        <button type="button" className="btn btn-icon" onClick={() => setWizardOpen(true)}>
          <Icon name="plus" size={16} /> Create cluster
        </button>
      </div>
      {error && <div className="error">{error}</div>}
      {expectDelete && (
        <p className="muted" style={{ marginTop: 0 }}>
          Deleting cluster… the list will update when the job finishes.
        </p>
      )}
      <div className="card">
        <table>
          <thead>
            <tr>
              <th>Name</th>
              <th>Status</th>
              <th>Arch</th>
              <th>Provider</th>
              <th>CP / Workers</th>
              <th>CPU</th>
              <th>Memory</th>
              <th>Disk</th>
              <th>Network</th>
              <th>CNI</th>
            </tr>
          </thead>
          <tbody>
            {list.map((c) => {
              const net = c.network_mode || (c.vip6 && c.vip ? 'dual-stack' : c.vip6 ? 'ipv6' : 'ipv4')
              const to = `/clusters/${c.id}`
              const kind = normalizeProviderKind(c.provider_kind)
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
                    <span className={`badge arch arch-${c.arch === 'arm64' ? 'arm64' : 'amd64'}`}>
                      {c.arch === 'arm64' ? 'arm64' : 'amd64'}
                    </span>
                  </td>
                  <td>
                    {c.provider_name ? (
                      <div className="cluster-provider-cell">
                        <div className="cluster-provider-name">
                          <span className={`badge kind kind-${kind}`}>
                            {formatProviderKind(kind)}
                          </span>
                          <span>{c.provider_name}</span>
                          <ProviderStatusBadge availability={c.provider_availability} />
                        </div>
                        <div className="muted cluster-provider-node">
                          {c.provider_node || '—'}
                        </div>
                      </div>
                    ) : (
                      <span className="badge error">missing</span>
                    )}
                  </td>
                  <td>{c.controlplanes} / {c.workers}</td>
                  <td><UsageBar metric={metrics[c.id]?.cpu} color="cpu" /></td>
                  <td><UsageBar metric={metrics[c.id]?.memory} color="memory" /></td>
                  <td><UsageBar metric={metrics[c.id]?.disk} color="disk" /></td>
                  <td>
                    <span className="badge">{net}</span>
                    <span className="muted" style={{ marginLeft: 8 }}>
                      {c.vip || c.vip6 || '—'}
                    </span>
                  </td>
                  <td>{c.cni}</td>
                </tr>
              )
            })}
          </tbody>
        </table>
        {list.length === 0 && (
          <p className="muted">{loaded ? 'No clusters. Create with M control planes (+ VIP if M&gt;1) and N workers.' : 'Loading clusters…'}</p>
        )}
      </div>

      <ClusterWizard open={wizardOpen} onClose={() => setWizardOpen(false)} />
    </div>
  )
}
