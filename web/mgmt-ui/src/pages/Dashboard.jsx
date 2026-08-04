import { useCallback, useEffect, useState } from 'react'
import { Link, useNavigate } from 'react-router-dom'
import { api } from '../api'
import { Icon } from '../components/Icons'

const BUSY = new Set(['deleting', 'provisioning', 'pending', 'upgrading'])

export default function Dashboard() {
  const nav = useNavigate()
  const [clusters, setClusters] = useState([])
  const [providers, setProviders] = useState([])
  const [health, setHealth] = useState(null)

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

  useEffect(() => {
    load()
  }, [load])

  useEffect(() => {
    const busy = clusters.some((c) => BUSY.has(c.status))
    if (!busy) return undefined
    const t = setInterval(load, 2000)
    return () => clearInterval(t)
  }, [clusters, load])

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
      <div className="card">
        <h2 className="card-title"><Icon name="clusters" size={18} /> Recent clusters</h2>
        {clusters.length === 0 ? (
          <p className="muted">No clusters yet. Configure a Proxmox provider, then create M CP + N workers.</p>
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
