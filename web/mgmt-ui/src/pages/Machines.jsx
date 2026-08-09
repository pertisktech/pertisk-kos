import { useCallback, useEffect, useMemo, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { api } from '../api'
import { Icon } from '../components/Icons'

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

  const filtered = useMemo(() => {
    const needle = q.trim().toLowerCase()
    if (!needle) return list
    return list.filter((m) => {
      const hay = [
        m.name,
        m.role,
        m.status,
        m.ip,
        m.ip6,
        m.cluster_name,
        m.provider_name,
        m.k8s_version,
        m.source,
      ]
        .filter(Boolean)
        .join(' ')
        .toLowerCase()
      return hay.includes(needle)
    })
  }, [list, q])

  return (
    <div>
      <div className="page-head">
        <h1>
          <Icon name="machines" size={22} /> Machines
        </h1>
        <button type="button" className="secondary btn-icon" onClick={load}>
          <Icon name="check" size={16} /> Refresh
        </button>
      </div>
      {error && <div className="error">{error}</div>}
      <div className="card" style={{ marginBottom: '1rem' }}>
        <label className="field">
          Filter
          <input
            value={q}
            onChange={(e) => setQ(e.target.value)}
            placeholder="name, cluster, IP, role…"
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
                    <span className={`badge ${m.status}`}>{m.status}</span>
                  </td>
                  <td className="mono-inline">{m.ip || m.ip6 || '—'}</td>
                  <td className="mono-inline">{m.k8s_version || '—'}</td>
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
