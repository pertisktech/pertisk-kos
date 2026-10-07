import { lazy, Suspense, useEffect, useState } from 'react'
import Drawer from '../../components/Drawer'
import { Icon } from '../../components/Icons'
import { applyResource, getResource } from './api'

const YamlEditor = lazy(() => import('../../components/YamlEditor'))

export default function ResourceDrawer({
  open,
  onClose,
  clusterId,
  kind,
  row,
  onDeleted,
  onLogs,
  onExec,
}) {
  const [tab, setTab] = useState('overview')
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')
  const [summary, setSummary] = useState(null)
  const [yaml, setYaml] = useState('')
  const [events, setEvents] = useState([])
  const [saving, setSaving] = useState(false)

  useEffect(() => {
    if (!open || !row || !clusterId || !kind) return undefined
    let cancelled = false
    setTab('overview')
    setLoading(true)
    setError('')
    getResource(clusterId, kind, row.namespace, row.name)
      .then((res) => {
        if (cancelled) return
        setSummary(res.summary || row)
        setYaml(res.yaml || '')
        setEvents(res.events || [])
      })
      .catch((e) => {
        if (!cancelled) {
          setError(e.message || 'failed to load')
          setSummary(row)
          setYaml('')
          setEvents([])
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [open, clusterId, kind, row])

  async function onApply() {
    if (!row) return
    setSaving(true)
    setError('')
    try {
      await applyResource(clusterId, kind, row.namespace, row.name, yaml)
      const res = await getResource(clusterId, kind, row.namespace, row.name)
      setSummary(res.summary || row)
      setYaml(res.yaml || yaml)
      setEvents(res.events || [])
    } catch (e) {
      setError(e.message || 'apply failed')
    } finally {
      setSaving(false)
    }
  }

  const title = row ? row.name : 'Resource'
  const subtitle = row
    ? `${kind}${row.namespace ? ` · ${row.namespace}` : ''}`
    : ''

  return (
    <Drawer
      open={open}
      onClose={onClose}
      title={title}
      subtitle={subtitle}
      width="min(640px, 98vw)"
      footer={
        tab === 'yaml' ? (
          <div className="drawer-footer-actions">
            <button type="button" className="secondary" onClick={onClose}>
              Close
            </button>
            <button type="button" className="btn" onClick={onApply} disabled={saving || kind === 'events'}>
              {saving ? 'Applying…' : 'Apply YAML'}
            </button>
          </div>
        ) : (
          <div className="drawer-footer-actions">
            {kind === 'pods' && row && (
              <>
                <button type="button" className="secondary btn-icon" onClick={() => onLogs?.(row)}>
                  <Icon name="logs" size={14} /> Logs
                </button>
                <button type="button" className="secondary btn-icon" onClick={() => onExec?.(row)}>
                  <Icon name="terminal" size={14} /> Exec
                </button>
              </>
            )}
            <button type="button" className="secondary" onClick={onClose}>
              Close
            </button>
          </div>
        )
      }
    >
      <div className="drawer-tabs">
        {['overview', 'yaml', 'events'].map((t) => (
          <button
            key={t}
            type="button"
            className={`tab-btn${tab === t ? ' active' : ''}`}
            onClick={() => setTab(t)}
          >
            {t === 'yaml' ? 'YAML' : t[0].toUpperCase() + t.slice(1)}
          </button>
        ))}
      </div>

      {error && <div className="error">{error}</div>}
      {loading && <p className="muted">Loading…</p>}

      {!loading && tab === 'overview' && summary && (
        <dl className="k8s-detail-dl">
          {Object.entries(summary)
            .filter(([k]) => !['selector', 'images', 'containers'].includes(k))
            .map(([k, v]) => (
              <div key={k} className="k8s-detail-row">
                <dt>{k}</dt>
                <dd className="mono-inline">
                  {typeof v === 'object' ? JSON.stringify(v) : String(v ?? '—')}
                </dd>
              </div>
            ))}
          {Array.isArray(summary.images) && summary.images.length > 0 && (
            <div className="k8s-detail-row">
              <dt>images</dt>
              <dd className="muted">{summary.images.join(', ')}</dd>
            </div>
          )}
        </dl>
      )}

      {!loading && tab === 'yaml' && (
        <div className="k8s-yaml-pane">
          {kind === 'secrets' && (
            <p className="muted" style={{ marginBottom: '0.75rem' }}>
              Secret values are redacted. Applying YAML that still contains <code>***</code> is blocked.
            </p>
          )}
          <Suspense fallback={<p className="muted">Loading editor…</p>}>
            <div className="yaml-editor-frame" style={{ height: 420 }}>
              <YamlEditor value={yaml} onChange={setYaml} path={`k8s-${kind}-${row?.name || 'res'}`} schema="kubeconfig" />
            </div>
          </Suspense>
        </div>
      )}

      {!loading && tab === 'events' && (
        <div className="table-wrap">
          <table>
            <thead>
              <tr>
                <th>Type</th>
                <th>Reason</th>
                <th>Message</th>
                <th>Age</th>
              </tr>
            </thead>
            <tbody>
              {events.length === 0 && (
                <tr>
                  <td colSpan={4} className="muted">
                    No related events
                  </td>
                </tr>
              )}
              {events.map((e) => (
                <tr key={`${e.namespace}/${e.name}`}>
                  <td>
                    <span className={`badge ${e.status === 'Warning' ? 'error' : 'ready'}`}>
                      {e.status}
                    </span>
                  </td>
                  <td className="mono-inline">{e.ready}</td>
                  <td className="muted">{e.message}</td>
                  <td className="muted">{e.age}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {onDeleted ? null : null}
    </Drawer>
  )
}
