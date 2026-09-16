import { useCallback, useEffect, useMemo, useState } from 'react'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { api } from '../api'
import { Icon } from '../components/Icons'
import PageHeader from '../components/PageHeader'
import { NodeStatusBadges } from '../components/NodeStatusBadges'
import { useMgmtRefresh } from '../hooks/useMgmtEvents'

export default function Machines() {
  const nav = useNavigate()
  const [params, setParams] = useSearchParams()
  const [list, setList] = useState([])
  const [error, setError] = useState('')
  const [q, setQ] = useState(() => params.get('q') || '')

  const load = useCallback(() => {
    api('/machines')
      .then((rows) => setList(Array.isArray(rows) ? rows : []))
      .catch((e) => setError(e.message || 'failed to load machines'))
  }, [])

  useEffect(() => {
    load()
  }, [load])

  useMgmtRefresh(load)

  useEffect(() => {
    const next = params.get('q') || ''
    setQ((prev) => (prev === next ? prev : next))
  }, [params])

  const filtered = useMemo(() => {
    const needle = q.trim().toLowerCase()
    if (!needle) return list
    return list.filter((m) => {
      const hay = [
        m.name,
        m.role,
        m.status,
        m.availability,
        m.ip,
        m.ip6,
        m.cluster_name,
        m.provider_name,
        m.k8s_version,
        m.os_version,
        m.source,
      ]
        .filter(Boolean)
        .join(' ')
        .toLowerCase()
      return hay.includes(needle)
    })
  }, [list, q])

  const online = list.filter((m) => m.availability === 'online').length
  const offline = list.filter((m) => m.availability === 'offline').length

  return (
    <div className="dash-page">
      <PageHeader
        title="Machines"
        description="Individual nodes running the immutable Pertisk node OS."
        actions={
          <div className="row-actions">
            {list.length > 0 && (
              <span className="status-badges">
                <span className="badge online">{online} online</span>
                {offline > 0 && <span className="badge offline">{offline} offline</span>}
              </span>
            )}
            <button type="button" className="secondary btn-icon" onClick={load}>
              <Icon name="refresh" size={16} /> Refresh
            </button>
          </div>
        }
      />
      {error && <div className="error">{error}</div>}
      <div className="card" style={{ marginBottom: '1rem' }}>
        <label className="field">
          Filter
          <input
            value={q}
            onChange={(e) => {
              const next = e.target.value
              setQ(next)
              const sp = new URLSearchParams(params)
              if (next) sp.set('q', next)
              else sp.delete('q')
              setParams(sp, { replace: true })
            }}
            placeholder="name, cluster, IP, online/offline…"
          />
        </label>
      </div>
      <div className="table-shell">
        <table>
          <thead>
            <tr>
              <th>Hostname</th>
              <th>Cluster</th>
              <th>Role</th>
              <th>Source</th>
              <th>Status</th>
              <th>K8s</th>
              <th>OS</th>
              <th>AK</th>
            </tr>
          </thead>
          <tbody>
            {filtered.map((m) => {
              const to = `/clusters/${m.cluster_id}/nodes/${m.id}`
              const control = m.role === 'controlplane' || m.role === 'control-plane'
              return (
                <tr
                  key={m.id}
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
                  <td>
                    <div className="identity-cell">
                      <span className="row-click-label">{m.name}</span>
                      <span className="identity-cell-sub">{m.ip || m.ip6 || '—'}</span>
                    </div>
                  </td>
                  <td>
                    <div>{m.cluster_name}</div>
                    <div className="muted" style={{ fontSize: '0.75rem' }}>
                      {m.provider_name || '—'} · {m.cluster_status}
                    </div>
                  </td>
                  <td>
                    <span className={`tag ${control ? 'tag-accent' : 'tag-outline'}`}>
                      {control ? 'Control plane' : m.role || 'Worker'}
                    </span>
                  </td>
                  <td className="muted">
                    {m.source === 'adopted' || m.source === 'baremetal'
                      ? m.source
                      : m.vmid != null
                        ? `${m.source || 'proxmox'} #${m.vmid}`
                        : m.source || '—'}
                  </td>
                  <td>
                    <NodeStatusBadges status={m.status} availability={m.availability} />
                  </td>
                  <td>
                    <span className="tag tag-mono">{m.k8s_version || '—'}</span>
                  </td>
                  <td>
                    <span className="tag tag-mono">{m.os_version || '—'}</span>
                  </td>
                  <td>
                    <span className={`badge ${m.ak_enrolled ? 'ready' : ''}`}>
                      {m.ak_enrolled ? 'enrolled' : '—'}
                    </span>
                  </td>
                </tr>
              )
            })}
          </tbody>
        </table>
        {filtered.length === 0 && (
          <p className="muted" style={{ margin: '0.75rem 1rem' }}>
            No machines. Create a cluster to populate inventory.
          </p>
        )}
      </div>
    </div>
  )
}
