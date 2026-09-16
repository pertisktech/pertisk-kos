import { useCallback, useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { api } from '../api'
import { Icon } from '../components/Icons'
import PageHeader from '../components/PageHeader'
import { useConfirm } from '../components/Confirm'
import ProviderWizard from '../components/ProviderWizard'
import { ProviderStatusBadge } from '../components/ProviderStatusBadge'
import { formatProviderKind, normalizeProviderKind } from '../components/ClusterMetaBadges'
import ResourceGauge, { GAUGE_BASE } from '../components/ResourceGauge'
import { useMgmtRefresh } from '../hooks/useMgmtEvents'

function formatProbe(r, kind) {
  const label =
    kind === 'vsphere'
      ? 'ESXi'
      : kind === 'nutanix'
        ? 'Nutanix'
        : kind === 'pertisk-vms'
          ? 'Pertisk VMs'
          : 'Proxmox'
  const hostWord = kind === 'proxmox' || kind === 'pertisk-vms' || !kind ? 'node' : 'host'
  const hostsWord = kind === 'proxmox' || kind === 'pertisk-vms' || !kind ? 'nodes' : 'hosts'
  const storageWord =
    kind === 'vsphere'
      ? 'datastore'
      : kind === 'nutanix'
        ? 'storage container'
        : kind === 'pertisk-vms'
          ? 'storage backend'
          : 'storage'
  const nodes = (r.nodes || []).map((n) => n.node).join(', ') || '(none)'
  const parts = [
    `${label} ${r.version || '?'} @ ${r.url}`,
    `${hostsWord}: ${nodes}`,
    r.node_ok
      ? `${hostWord} OK (${r.node_message || 'ok'})`
      : `${hostWord} FAIL: ${r.node_message || 'unknown'}`,
  ]
  if (r.arch) {
    parts.push(`guest arch → ${r.arch}`)
  }
  if (r.storage) {
    parts.push(
      r.storage.ok
        ? `${storageWord} OK: ${r.storage.storage} (${r.storage.type_ || r.storage.type || '?'})`
        : `${storageWord} FAIL: ${r.storage.message}`,
    )
  }
  return parts.join(' — ')
}

const emptyMetric = {
  used: null,
  total: null,
  percent: null,
  unit: '',
  display_used: null,
  display_total: null,
  error: null,
}

export default function Providers() {
  const confirm = useConfirm()
  const [list, setList] = useState([])
  const [metrics, setMetrics] = useState({})
  const [wizardOpen, setWizardOpen] = useState(false)
  const [wizardMode, setWizardMode] = useState('create')
  const [editing, setEditing] = useState(null)
  const [error, setError] = useState('')
  const [msg, setMsg] = useState('')
  const [testing, setTesting] = useState(false)

  const load = useCallback(() => {
    Promise.all([
      api('/providers'),
      api('/dashboard/providers').catch(() => []),
    ])
      .then(([rows, res]) => {
        setError('')
        setList(Array.isArray(rows) ? rows : [])
        const map = {}
        for (const r of Array.isArray(res) ? res : []) {
          if (r?.provider_id) map[r.provider_id] = r
        }
        setMetrics(map)
      })
      .catch((e) => setError(e.message))
  }, [])
  useEffect(() => {
    load()
  }, [load])
  useMgmtRefresh(load)

  function startCreate() {
    setError('')
    setMsg('')
    setWizardMode('create')
    setEditing(null)
    setWizardOpen(true)
  }

  function startEdit(p) {
    setError('')
    setMsg('')
    setWizardMode('edit')
    setEditing(p)
    setWizardOpen(true)
  }

  async function testSaved(id) {
    setError('')
    setMsg('Testing…')
    setTesting(true)
    try {
      const r = await api(`/providers/${id}/test`, { method: 'POST', body: {} })
      const p = list.find((x) => x.id === id)
      const text = formatProbe(r, p?.kind || 'proxmox')
      if (r.ok) {
        setMsg(`OK — ${text}`)
        setError('')
      } else {
        setMsg('')
        setError(text)
      }
    } catch (err) {
      setMsg('')
      setError(err.message)
    } finally {
      setTesting(false)
    }
  }

  async function remove(id, name) {
    const ok = await confirm({
      title: 'Delete provider',
      message: `Remove provider “${name}”? Clusters that use it will keep records but cannot recreate VMs.`,
      confirmLabel: 'Delete',
      tone: 'danger',
    })
    if (!ok) return
    setError('')
    try {
      await api(`/providers/${id}`, { method: 'DELETE' })
      if (editing?.id === id) {
        setWizardOpen(false)
        setEditing(null)
      }
      load()
    } catch (err) {
      setError(err.message)
    }
  }

  return (
    <div className="dash-page">
      <PageHeader
        title="Providers"
        description="Hypervisors and bare-metal backends supplying compute to the fleet."
        actions={
          <button type="button" className="btn-icon" onClick={startCreate}>
            <Icon name="plus" size={16} /> Connect provider
          </button>
        }
      />
      {error && <div className="error">{error}</div>}
      {msg && <p className="muted">{msg}</p>}

      {list.length === 0 ? (
        <div className="card dash-empty">
          <p className="muted" style={{ margin: 0 }}>
            No providers configured.
          </p>
        </div>
      ) : (
        <section className="entity-grid entity-grid-2">
          {list.map((p) => {
            const kind = normalizeProviderKind(p.kind)
            const live = metrics[p.id] || {}
            return (
              <article key={p.id} className="entity-card">
                <div className="entity-card-head">
                  <Link to={`/providers/${p.id}`} className="entity-card-identity">
                    <span className="entity-card-icon" aria-hidden>
                      <Icon name="providers" size={18} />
                    </span>
                    <div className="entity-card-copy">
                      <p className="entity-card-name">{p.name}</p>
                      <p className="entity-card-sub">{p.url}</p>
                    </div>
                  </Link>
                  <ProviderStatusBadge availability={p.availability} showUnknown />
                </div>

                <div className="entity-card-tags">
                  <span className={`badge kind kind-${kind}`}>{formatProviderKind(p.kind)}</span>
                  <span className="tag tag-mono">{p.arch || 'amd64'}</span>
                  <span className="tag tag-outline">{p.insecure ? 'insecure TLS' : 'verify TLS'}</span>
                </div>

                <div className="entity-card-stats">
                  <div className="entity-card-stat">
                    <p className="entity-card-stat-label">Host</p>
                    <p className="entity-card-stat-value">{p.node || '—'}</p>
                  </div>
                  <div className="entity-card-stat">
                    <p className="entity-card-stat-label">Storage</p>
                    <p className="entity-card-stat-value">{p.storage || '—'}</p>
                  </div>
                </div>

                <div className="cluster-card-meters">
                  <ResourceGauge
                    label="CPU"
                    icon="cpu"
                    metric={live.cpu || { ...emptyMetric, unit: 'cores' }}
                    color={GAUGE_BASE.cpu}
                    size="sm"
                  />
                  <ResourceGauge
                    label="Memory"
                    icon="memory"
                    metric={live.memory || { ...emptyMetric, unit: 'GiB' }}
                    color={GAUGE_BASE.memory}
                    size="sm"
                  />
                  <ResourceGauge
                    label="Disk"
                    icon="disk"
                    metric={live.disk || { ...emptyMetric, unit: 'GiB' }}
                    color={GAUGE_BASE.disk}
                    size="sm"
                  />
                </div>
                {live.error ? (
                  <p className="muted cluster-resource-soft-err" title={live.error}>
                    <Icon name="alert" size={12} />
                    {live.error}
                  </p>
                ) : null}

                <div className="entity-card-actions">
                  <Link className="btn secondary btn-icon" to={`/providers/${p.id}`}>
                    <Icon name="dashboard" size={14} /> Dashboard
                  </Link>
                  <button type="button" className="secondary btn-icon" onClick={() => startEdit(p)}>
                    <Icon name="edit" size={14} /> Edit
                  </button>
                  <button
                    type="button"
                    className="secondary btn-icon"
                    onClick={() => testSaved(p.id)}
                    disabled={testing}
                  >
                    <Icon name="play" size={14} /> Test
                  </button>
                  <button
                    type="button"
                    className="danger btn-icon"
                    onClick={() => remove(p.id, p.name)}
                  >
                    <Icon name="trash" size={14} />
                  </button>
                </div>
              </article>
            )
          })}
        </section>
      )}

      <ProviderWizard
        open={wizardOpen}
        mode={wizardMode}
        provider={editing}
        onClose={() => {
          setWizardOpen(false)
          setEditing(null)
        }}
        onSaved={() => {
          setMsg(wizardMode === 'edit' ? 'Provider updated' : 'Provider created')
          load()
        }}
      />
    </div>
  )
}
