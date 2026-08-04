import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { api } from '../api'

export default function Clusters() {
  const [list, setList] = useState([])
  const [error, setError] = useState('')

  useEffect(() => {
    api('/clusters').then(setList).catch((e) => setError(e.message))
  }, [])

  return (
    <div>
      <div className="page-head">
        <h1>Clusters</h1>
        <Link className="btn" to="/clusters/new">Create cluster</Link>
      </div>
      {error && <div className="error">{error}</div>}
      <div className="card">
        <table>
          <thead>
            <tr>
              <th>Name</th>
              <th>Status</th>
              <th>CP / Workers</th>
              <th>VIP</th>
              <th>CNI</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {list.map((c) => (
              <tr key={c.id}>
                <td>{c.name}</td>
                <td><span className={`badge ${c.status}`}>{c.status}</span></td>
                <td>{c.controlplanes} / {c.workers}</td>
                <td>{c.vip || '—'}</td>
                <td>{c.cni}</td>
                <td><Link to={`/clusters/${c.id}`}>Details</Link></td>
              </tr>
            ))}
          </tbody>
        </table>
        {list.length === 0 && (
          <p className="muted">No clusters. Create with M control planes (+ VIP if M&gt;1) and N workers.</p>
        )}
      </div>
    </div>
  )
}
