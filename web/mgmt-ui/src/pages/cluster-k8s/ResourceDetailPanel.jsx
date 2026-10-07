import { lazy, Suspense, useEffect, useState } from 'react'
import { Icon } from '../../components/Icons'
import { applyResource, getResource, restartDeployment, scaleDeployment } from './api'

const YamlEditor = lazy(() => import('../../components/YamlEditor'))

function PanelAction({ icon, label, onClick, danger, disabled }) {
  return (
    <button
      type="button"
      className={`k8s-panel-action${danger ? ' is-danger' : ''}`}
      title={label}
      aria-label={label}
      disabled={disabled}
      onClick={onClick}
    >
      <Icon name={icon} size={16} />
    </button>
  )
}

function statusTone(status) {
  const s = (status || '').toLowerCase()
  if (['running', 'active', 'complete', 'succeeded', 'ready', 'bound'].includes(s)) return 'ok'
  if (['pending', 'progressing', 'suspended', 'warning'].includes(s)) return 'warn'
  if (['failed', 'stopped', 'error', 'crashloopbackoff', 'notready'].includes(s)) return 'bad'
  return ''
}

export default function ResourceDetailPanel({
  clusterId,
  kind,
  row,
  onClose,
  onDeleted,
  onLogs,
  onExec,
  onMutated,
}) {
  const [section, setSection] = useState('overview') // overview | yaml
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')
  const [summary, setSummary] = useState(null)
  const [yaml, setYaml] = useState('')
  const [events, setEvents] = useState([])
  const [saving, setSaving] = useState(false)
  const [replicas, setReplicas] = useState(1)
  const [scaling, setScaling] = useState(false)

  useEffect(() => {
    if (!row || !clusterId || !kind) return undefined
    let cancelled = false
    setSection('overview')
    setLoading(true)
    setError('')
    getResource(clusterId, kind, row.namespace, row.name)
      .then((res) => {
        if (cancelled) return
        const s = res.summary || row
        setSummary(s)
        setYaml(res.yaml || '')
        setEvents(res.events || [])
        setReplicas(Number(s.replicas ?? 1) || 0)
      })
      .catch((e) => {
        if (!cancelled) {
          setError(e.message || 'failed to load')
          setSummary(row)
          setYaml('')
          setEvents([])
          setReplicas(Number(row.replicas ?? 1) || 0)
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [clusterId, kind, row])

  async function reload() {
    const res = await getResource(clusterId, kind, row.namespace, row.name)
    setSummary(res.summary || row)
    setYaml(res.yaml || '')
    setEvents(res.events || [])
    setReplicas(Number(res.summary?.replicas ?? row.replicas ?? 1) || 0)
    onMutated?.()
  }

  async function onApply() {
    setSaving(true)
    setError('')
    try {
      await applyResource(clusterId, kind, row.namespace, row.name, yaml)
      await reload()
      setSection('overview')
    } catch (e) {
      setError(e.message || 'apply failed')
    } finally {
      setSaving(false)
    }
  }

  async function onScaleApply() {
    setScaling(true)
    setError('')
    try {
      await scaleDeployment(clusterId, row.namespace, row.name, replicas)
      await reload()
    } catch (e) {
      setError(e.message || 'scale failed')
    } finally {
      setScaling(false)
    }
  }

  async function onRestart() {
    setError('')
    try {
      await restartDeployment(clusterId, row.namespace, row.name)
      await reload()
    } catch (e) {
      setError(e.message || 'restart failed')
    }
  }

  if (!row) return null

  const s = summary || row
  const kindLabel = String(kind || '').replace(/s$/, '').toUpperCase() || 'RESOURCE'
  const canDelete = kind !== 'events' && kind !== 'nodes'
  const isDeploy = kind === 'deployments'
  const isPods = kind === 'pods'

  return (
    <aside className="k8s-detail-panel" aria-label="Resource details">
      <header className="k8s-detail-header">
        <div className="k8s-detail-heading">
          <div className="k8s-detail-kind">
            <Icon name="cpu" size={16} />
            <span>{kindLabel}</span>
          </div>
          <h2 className="k8s-detail-title" title={row.name}>
            {row.name}
          </h2>
          {s.status && (
            <span className={`k8s-status ${statusTone(s.status)}`}>{s.status}</span>
          )}
        </div>
        <div className="k8s-detail-actions">
          {isPods && (
            <>
              <PanelAction icon="logs" label="Logs" onClick={() => onLogs?.(row)} />
              <PanelAction icon="terminal" label="Exec" onClick={() => onExec?.(row)} />
            </>
          )}
          {isDeploy && (
            <PanelAction icon="reboot" label="Restart" onClick={onRestart} />
          )}
          <PanelAction
            icon="edit"
            label="Edit YAML"
            onClick={() => setSection((cur) => (cur === 'yaml' ? 'overview' : 'yaml'))}
          />
          {canDelete && (
            <PanelAction icon="trash" label="Delete" danger onClick={() => onDeleted?.(row)} />
          )}
          <PanelAction icon="x" label="Close" onClick={onClose} />
        </div>
      </header>

      <div className="k8s-detail-keyinfo">
        {row.namespace ? (
          <div>
            <p>Namespace</p>
            <strong>{row.namespace}</strong>
          </div>
        ) : null}
        <div>
          <p>Ready</p>
          <strong>{s.ready || '—'}</strong>
        </div>
        <div>
          <p>Age</p>
          <strong>{s.age || '—'}</strong>
        </div>
        {s.restarts != null && (
          <div>
            <p>Restarts</p>
            <strong>{s.restarts}</strong>
          </div>
        )}
      </div>

      <div className="k8s-detail-body">
        {error && <div className="error">{error}</div>}
        {loading && <p className="muted">Loading…</p>}

        {!loading && section === 'yaml' && (
          <section className="k8s-detail-section">
            <h3 className="k8s-detail-section-title">YAML</h3>
            {kind === 'secrets' && (
              <p className="muted k8s-detail-hint">
                Secret values are redacted. Applying YAML that still contains <code>***</code> is blocked.
              </p>
            )}
            <Suspense fallback={<p className="muted">Loading editor…</p>}>
              <div className="yaml-editor-frame" style={{ height: 360 }}>
                <YamlEditor
                  value={yaml}
                  onChange={setYaml}
                  path={`k8s-${kind}-${row.name}`}
                  schema="kubeconfig"
                />
              </div>
            </Suspense>
            <div className="k8s-detail-yaml-actions">
              <button type="button" className="secondary" onClick={() => setSection('overview')}>
                Cancel
              </button>
              <button type="button" className="btn" onClick={onApply} disabled={saving || kind === 'events'}>
                {saving ? 'Applying…' : 'Apply'}
              </button>
            </div>
          </section>
        )}

        {!loading && section === 'overview' && (
          <>
            {isDeploy && (
              <section className="k8s-detail-section">
                <h3 className="k8s-detail-section-title">Scale</h3>
                <div className="k8s-scale-row">
                  <button
                    type="button"
                    className="k8s-icon-btn"
                    onClick={() => setReplicas((n) => Math.max(0, n - 1))}
                    aria-label="Decrease replicas"
                  >
                    −
                  </button>
                  <input
                    type="number"
                    min={0}
                    value={replicas}
                    onChange={(e) => setReplicas(Math.max(0, Number(e.target.value) || 0))}
                  />
                  <button
                    type="button"
                    className="k8s-icon-btn"
                    onClick={() => setReplicas((n) => n + 1)}
                    aria-label="Increase replicas"
                  >
                    +
                  </button>
                  <button type="button" className="btn" onClick={onScaleApply} disabled={scaling}>
                    {scaling ? 'Applying…' : 'Apply'}
                  </button>
                </div>
              </section>
            )}

            <section className="k8s-detail-section">
              <h3 className="k8s-detail-section-title">Properties</h3>
              <dl className="k8s-prop-list">
                {Object.entries(s)
                  .filter(([k]) => !['selector', 'images', 'containers', 'kind'].includes(k))
                  .map(([k, v]) => (
                    <div key={k} className="k8s-prop-row">
                      <dt>{k}</dt>
                      <dd>{typeof v === 'object' ? JSON.stringify(v) : String(v ?? '—')}</dd>
                    </div>
                  ))}
                {Array.isArray(s.images) && s.images.length > 0 && (
                  <div className="k8s-prop-row">
                    <dt>images</dt>
                    <dd>{s.images.map((img) => String(img).split('@')[0]).join(', ')}</dd>
                  </div>
                )}
              </dl>
            </section>

            <section className="k8s-detail-section">
              <h3 className="k8s-detail-section-title">Events ({events.length})</h3>
              {events.length === 0 ? (
                <p className="muted">No recent events</p>
              ) : (
                <div className="k8s-events-list">
                  {events.map((e) => (
                    <article key={`${e.namespace}/${e.name}`} className="k8s-event-card">
                      <div className="k8s-event-top">
                        <span className={`k8s-status ${e.status === 'Warning' ? 'bad' : 'ok'}`}>
                          {e.ready || e.status}
                        </span>
                        <span className="muted">{e.age}</span>
                      </div>
                      <p>{e.message || '—'}</p>
                    </article>
                  ))}
                </div>
              )}
            </section>
          </>
        )}
      </div>
    </aside>
  )
}
