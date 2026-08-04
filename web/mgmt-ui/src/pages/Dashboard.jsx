import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { api } from '../api'
import { Icon } from '../components/Icons'

export default function Dashboard() {
  const [clusters, setClusters] = useState([])
  const [providers, setProviders] = useState([])
  const [health, setHealth] = useState(null)

  useEffect(() => {
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
              <tr><th>Name</th><th>Status</th><th>Topology</th><th></th></tr>
            </thead>
            <tbody>
              {clusters.slice(0, 8).map((c) => (
                <tr key={c.id}>
                  <td>{c.name}</td>
                  <td><span className={`badge ${c.status}`}>{c.status}</span></td>
                  <td>{c.controlplanes} CP / {c.workers} WK{c.vip ? ` · VIP ${c.vip}` : ''}</td>
                  <td><Link to={`/clusters/${c.id}`}>Open</Link></td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  )
}
