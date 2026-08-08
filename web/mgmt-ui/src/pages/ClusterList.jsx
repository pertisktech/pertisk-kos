import { useCallback, useEffect, useState } from 'react'
import { Link, useNavigate, useSearchParams } from 'react-router-dom'
import { api } from '../api'
import { Icon } from '../components/Icons'
import { ClusterStatusBadges } from '../components/ClusterStatusBadges'

const BUSY = new Set(['deleting', 'provisioning', 'pending', 'upgrading'])
const AVAIL_POLL_MS = 15000

export default function Clusters() {
  const nav = useNavigate()
  const [list, setList] = useState([])
  const [error, setError] = useState('')
  const [search, setSearch] = useSearchParams()
  const expectDelete = search.get('deleting')

  const load = useCallback(() => {
    api('/clusters')
      .then((rows) => {
        setList(rows)
        // Drop ?deleting= once that cluster is gone from the API.
        if (expectDelete && !rows.some((c) => c.id === expectDelete)) {
          setSearch({}, { replace: true })
        }
      })
      .catch((e) => setError(e.message))
  }, [expectDelete, setSearch])

  useEffect(() => {
    load()
  }, [load])

  // Keep polling while any cluster is mid-create/delete, or until a just-deleted id disappears.
  useEffect(() => {
    const busy = list.some((c) => BUSY.has(c.status)) || !!expectDelete
    if (!busy) return undefined
    const t = setInterval(load, 2000)
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
        <Link className="btn btn-icon" to="/clusters/new">
          <Icon name="plus" size={16} /> Create cluster
        </Link>
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
              <th>Provider</th>
              <th>CP / Workers</th>
              <th>Network</th>
              <th>CNI</th>
            </tr>
          </thead>
          <tbody>
            {list.map((c) => {
              const net = c.network_mode || (c.vip6 && c.vip ? 'dual-stack' : c.vip6 ? 'ipv6' : 'ipv4')
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
                  <td>
                    <ClusterStatusBadges status={c.status} availability={c.availability} />
                  </td>
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
