import { useCallback, useEffect, useState } from 'react'
import { api } from '../api'
import { Icon } from '../components/Icons'

const PAGE = 100

export default function Audit() {
  const [rows, setRows] = useState([])
  const [error, setError] = useState('')
  const [action, setAction] = useState('')
  const [resource, setResource] = useState('')
  const [offset, setOffset] = useState(0)

  const load = useCallback(() => {
    const q = new URLSearchParams()
    q.set('limit', String(PAGE))
    q.set('offset', String(offset))
    if (action.trim()) q.set('action', action.trim())
    if (resource.trim()) q.set('resource', resource.trim())
    api(`/audit?${q}`)
      .then((data) => setRows(Array.isArray(data) ? data : []))
      .catch((e) => setError(e.message || 'failed to load audit'))
  }, [action, resource, offset])

  useEffect(() => {
    load()
  }, [load])

  return (
    <div>
      <div className="page-head">
        <h1>
          <Icon name="audit" size={22} /> Audit
        </h1>
        <button type="button" className="secondary btn-icon" onClick={load}>
          <Icon name="check" size={16} /> Refresh
        </button>
      </div>
      {error && <div className="error">{error}</div>}
      <div className="card" style={{ marginBottom: '1rem' }}>
        <div className="form-row" style={{ display: 'flex', gap: '0.75rem', flexWrap: 'wrap' }}>
          <label className="field" style={{ flex: '1 1 12rem' }}>
            Action
            <input
              value={action}
              onChange={(e) => {
                setOffset(0)
                setAction(e.target.value)
              }}
              placeholder="e.g. cluster.config"
            />
          </label>
          <label className="field" style={{ flex: '1 1 12rem' }}>
            Resource
            <input
              value={resource}
              onChange={(e) => {
                setOffset(0)
                setResource(e.target.value)
              }}
              placeholder="cluster / template id"
            />
          </label>
        </div>
      </div>
      <div className="card">
        <table>
          <thead>
            <tr>
              <th>Time</th>
              <th>User</th>
              <th>Action</th>
              <th>Resource</th>
              <th>Detail</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((r) => (
              <tr key={r.id}>
                <td className="mono-inline" style={{ whiteSpace: 'nowrap' }}>
                  {r.created_at}
                </td>
                <td>{r.username || r.user_id || '—'}</td>
                <td>
                  <span className="badge">{r.action}</span>
                </td>
                <td className="mono-inline">{r.resource || '—'}</td>
                <td className="muted" title={r.detail || ''}>
                  {r.detail
                    ? r.detail.length > 80
                      ? `${r.detail.slice(0, 80)}…`
                      : r.detail
                    : '—'}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        {rows.length === 0 && <p className="muted">No audit entries.</p>}
        <div className="form-footer" style={{ marginTop: '0.75rem', gap: '0.5rem' }}>
          <button
            type="button"
            className="secondary"
            disabled={offset === 0}
            onClick={() => setOffset((o) => Math.max(0, o - PAGE))}
          >
            Previous
          </button>
          <button
            type="button"
            className="secondary"
            disabled={rows.length < PAGE}
            onClick={() => setOffset((o) => o + PAGE)}
          >
            Next
          </button>
        </div>
      </div>
    </div>
  )
}
