import { useCallback, useEffect, useMemo, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { api } from '../api'
import { Icon } from '../components/Icons'
import { NodeStatusBadges } from '../components/NodeStatusBadges'

/** Refresh live online/offline while the page is open. */
const AVAIL_POLL_MS = 15_000

export default function Machines() {
  const nav = useNavigate()
  const [list, setList] = useState([])
  const [error, setError] = useState('')
  const [q, setQ] = useState('')

  const load = useCallback(() => {
    api('/machines')
      .then((rows) => setList(Array.isArray(rows) ? rows : []))
      .catch((e) => setError(e.message || 'failed to load machines'))
  }, [])

  useEffect(() => {
    load()
  }, [load])

  useEffect(() => {
    const t = setInterval(load, AVAIL_POLL_MS)
    return () => clearInterval(t)
  }, [load])

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
    <div>
      <div className="page-head">
        <h1>
          <Icon name="machines" size={22} /> Machines
        </h1>
        <div className="row-actions">
          {list.length > 0 && (
            <span className="status-badges" style={{ marginRight: 8 }}>
              <span className="badge online">{online} online</span>
              {offline > 0 && <span className="badge offline">{offline} offline</span>}
            </span>
          )}
          <button type="button" className="secondary btn-icon" onClick={load}>
            <Icon name="check" size={16} /> Refresh
          </button>
        </div>
      </div>
      {error && <div className="error">{error}</div>}
      <div className="card" style={{ marginBottom: '1rem' }}>
        <label className="field">
          Filter
          <input
            value={q}
            onChange={(e) => setQ(e.target.value)}
            placeholder="name, cluster, IP, online/offline…"
          />
        </label>
      </div>
      <div className="card">
        <table>
          <thead>
            <tr>
              <th>Name</th>
              <th>Cluster</th>
              <th>Role</th>
              <th>Source</th>
              <th>Status</th>
              <th>IP</th>
              <th>K8s</th>
              <th>OS</th>
              <th>AK</th>
            </tr>
          </thead>
          <tbody>
            {filtered.map((m) => {
              const to = `/clusters/${m.cluster_id}/nodes/${m.id}`
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
                    <span className="row-click-label">{m.name}</span>
                  </td>
                  <td>
                    <div>{m.cluster_name}</div>
                    <div className="muted" style={{ fontSize: '0.75rem' }}>
                      {m.provider_name || '—'} · {m.cluster_status}
                    </div>
                  </td>
                  <td>
                    <span className="badge">{m.role}</span>
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
                  <td className="mono-inline">{m.ip || m.ip6 || '—'}</td>
                  <td className="mono-inline">{m.k8s_version || '—'}</td>
                  <td className="mono-inline">{m.os_version || '—'}</td>
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
          <p className="muted">No machines. Create a cluster to populate inventory.</p>
        )}
      </div>
    </div>
  )
}
