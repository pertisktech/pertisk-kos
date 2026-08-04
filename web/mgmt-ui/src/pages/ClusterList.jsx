import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { api } from '../api'
import { Icon } from '../components/Icons'

export default function Clusters() {
  const [list, setList] = useState([])
  const [error, setError] = useState('')

  useEffect(() => {
    api('/clusters').then(setList).catch((e) => setError(e.message))
  }, [])

  return (
    <div>
      <div className="page-head">
        <h1><Icon name="clusters" size={22} /> Clusters</h1>
        <Link className="btn btn-icon" to="/clusters/new">
          <Icon name="plus" size={16} /> Create cluster
        </Link>
      </div>
      {error && <div className="error">{error}</div>}
      <div className="card">
        <table>
          <thead>
            <tr>
              <th>Name</th>
              <th>Status</th>
              <th>Provider</th>
              <th>CP / Workers</th>
              <th>Network</th>
              <th>CNI</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {list.map((c) => {
              const net = c.network_mode || (c.vip6 && c.vip ? 'dual-stack' : c.vip6 ? 'ipv6' : 'ipv4')
              return (
                <tr key={c.id}>
                  <td>{c.name}</td>
                  <td><span className={`badge ${c.status}`}>{c.status}</span></td>
                  <td>
                    {c.provider_name ? (
                      <div>
                        <div>{c.provider_name}</div>
                        <div className="muted" style={{ fontSize: '0.75rem' }}>
                          {c.provider_node || '—'}
                        </div>
                      </div>
                    ) : (
                      <span className="badge error">missing</span>
                    )}
                  </td>
                  <td>{c.controlplanes} / {c.workers}</td>
                  <td>
                    <span className="badge">{net}</span>
                    <span className="muted" style={{ marginLeft: 8 }}>
                      {c.vip || c.vip6 || '—'}
                    </span>
                  </td>
                  <td>{c.cni}</td>
                  <td><Link to={`/clusters/${c.id}`}>Details</Link></td>
                </tr>
              )
            })}
          </tbody>
        </table>
        {list.length === 0 && (
          <p className="muted">No clusters. Create with M control planes (+ VIP if M&gt;1) and N workers.</p>
        )}
      </div>
    </div>
  )
}
