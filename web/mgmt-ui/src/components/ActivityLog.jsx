import { useCallback, useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { api } from '../api'
import { useMgmtRefresh } from '../hooks/useMgmtEvents'

function toneFor(row) {
  const hay = `${row.action || ''} ${row.detail || ''} ${row.resource || ''}`.toLowerCase()
  if (/\bfail|\berror|\bdenied|\bunauthor|\bdegraded/.test(hay)) return 'err'
  if (/\bwarn|\bdrain|\brollback|\bupgrade|\bprovision/.test(hay)) return 'warn'
  if (/\bcreate|\bready|\binstall|\bjoin|\bok\b|\bsuccess/.test(hay)) return 'ok'
  return 'muted'
}

function formatTime(iso) {
  if (!iso) return '—'
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) {
    // already a display string from sqlite
    const m = String(iso).match(/(\d{2}:\d{2}:\d{2})/)
    return m ? m[1] : String(iso).slice(-8)
  }
  return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false })
}

function formatMsg(row) {
  const action = row.action || 'event'
  const resource = row.resource ? ` ${row.resource}` : ''
  const detail = row.detail ? ` — ${row.detail}` : ''
  const who = row.username ? ` (${row.username})` : ''
  return `${action}${resource}${detail}${who}`
}

/**
 * Fleet activity feed (redesign Activity panel) backed by audit log.
 */
export default function ActivityLog({ limit = 24 }) {
  const [rows, setRows] = useState([])
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(true)

  const load = useCallback(() => {
    api(`/audit?limit=${limit}&offset=0`)
      .then((data) => {
        setRows(Array.isArray(data) ? data : [])
        setError('')
      })
      .catch((e) => setError(e.message || 'failed to load activity'))
      .finally(() => setLoading(false))
  }, [limit])

  useEffect(() => {
    load()
  }, [load])

  useMgmtRefresh(load)

  return (
    <section className="fleet-panel activity-panel">
      <div className="fleet-panel-head">
        <div className="fleet-panel-title-row">
          <h2 className="fleet-panel-title">Activity</h2>
          <Link to="/audit" className="section-link">
            Audit
          </Link>
        </div>
      </div>
      <div className="activity-body">
        {error && <div className="error activity-err">{error}</div>}
        {loading && rows.length === 0 ? (
          <div className="activity-empty muted">Loading activity…</div>
        ) : rows.length === 0 ? (
          <div className="activity-empty muted">No recent events.</div>
        ) : (
          <div className="activity-list">
            {rows.map((row) => (
              <div key={row.id} className="activity-row">
                <span className="activity-time">{formatTime(row.created_at)}</span>
                <span className={`activity-msg ${toneFor(row)}`}>{formatMsg(row)}</span>
              </div>
            ))}
            <div className="activity-cursor" aria-hidden>
              <span className="activity-prompt">$</span>
              <span className="muted">tail -f /var/log/pertiskd/events.log</span>
              <span className="activity-caret" />
            </div>
          </div>
        )}
      </div>
    </section>
  )
}
