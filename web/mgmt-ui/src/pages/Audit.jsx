import { useCallback, useEffect, useState } from 'react'
import { api } from '../api'
import { Icon } from '../components/Icons'
import PageHeader from '../components/PageHeader'

const PAGE = 100

function isFailed(row) {
  const hay = `${row.action || ''} ${row.detail || ''}`.toLowerCase()
  return /\bfail|\berror|\bdenied|\bunauthor/.test(hay)
}

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
    <div className="dash-page">
      <PageHeader
        title="Audit log"
        description="Immutable record of every action taken against the control plane."
        actions={
          <button type="button" className="secondary btn-icon" onClick={load}>
            <Icon name="refresh" size={16} /> Refresh
          </button>
        }
      />
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
      <div className="card audit-card">
        {rows.length === 0 ? (
          <p className="muted" style={{ margin: '0.75rem' }}>
            No audit entries.
          </p>
        ) : (
          <ol className="audit-timeline">
            {rows.map((r, index) => {
              const failed = isFailed(r)
              return (
                <li key={r.id} className="audit-item">
                  <div className="audit-rail">
                    <span className={`audit-dot ${failed ? 'audit-dot-fail' : 'audit-dot-ok'}`}>
                      <Icon name={failed ? 'x' : 'check'} size={14} />
                    </span>
                    {index < rows.length - 1 ? <span className="audit-line" /> : null}
                  </div>
                  <div className="audit-body">
                    <div className="audit-title">
                      <code className="audit-action">{r.action}</code>
                      <span className="audit-target">{r.resource || '—'}</span>
                    </div>
                    <p className="audit-meta">
                      <span className="audit-actor">{r.username || r.user_id || '—'}</span>
                      {' · '}
                      {r.created_at}
                    </p>
                    {r.detail ? (
                      <p className="audit-detail" title={r.detail}>
                        {r.detail.length > 120 ? `${r.detail.slice(0, 120)}…` : r.detail}
                      </p>
                    ) : null}
                  </div>
                </li>
              )
            })}
          </ol>
        )}
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
