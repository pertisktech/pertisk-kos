import { useCallback, useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { api } from '../api'
import { Icon } from '../components/Icons'
import PageHeader from '../components/PageHeader'
import StatCard from '../components/StatCard'
import { useConfirm } from '../components/Confirm'
import ProviderWizard from '../components/ProviderWizard'
import { formatProviderKind, providerKindGlyph } from '../components/ClusterMetaBadges'
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

function formatMetric(m) {
  if (!m) return '—'
  const used = m.display_used ?? m.used
  const total = m.display_total ?? m.total
  if (used == null && total == null) return '—'
  const unit = m.unit ? ` ${m.unit}` : ''
  if (total == null) return `${used}${unit}`
  if (used == null) return `— / ${total}${unit}`
  return `${used} / ${total}${unit}`
}

function availTone(availability) {
  if (availability === 'online') return 'ok'
  if (availability === 'offline') return 'err'
  return 'warn'
}

function availLabel(availability) {
  if (availability === 'online') return 'Connected'
  if (availability === 'offline') return 'Offline'
  if (!availability) return 'Unknown'
  return availability.charAt(0).toUpperCase() + availability.slice(1)
}

function CompactProviderRow({
  provider,
  live,
  clusterCount,
  machineCount,
  onOpen,
  onEdit,
  onTest,
  onRemove,
  testing,
}) {
  const tone = availTone(provider.availability)
  const kind = formatProviderKind(provider.kind)
  const desc = [kind, provider.node, provider.arch || 'amd64'].filter(Boolean).join(' · ')

  return (
    <div
      className="compact-cluster-row compact-provider-row"
      role="link"
      tabIndex={0}
      onClick={onOpen}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault()
          onOpen()
        }
      }}
    >
      <div className="compact-cluster-identity">
        <span className={`compact-cluster-icon entity-card-glyph ${tone}`} aria-hidden>
          {providerKindGlyph(provider.kind)}
        </span>
        <div style={{ minWidth: 0 }}>
          <div className="compact-cluster-name">{provider.name}</div>
          <div className="compact-cluster-desc">{provider.url || desc}</div>
        </div>
      </div>
      <div>
        <span className="compact-cluster-mobile-label">Status</span>
        <span className={`compact-cluster-status ${tone}`}>
          <span className="dot" aria-hidden />
          {availLabel(provider.availability)}
        </span>
      </div>
      <div>
        <span className="compact-cluster-mobile-label">Clusters</span>
        <div className="compact-cluster-cell">{clusterCount}</div>
        <div className="compact-cluster-cell-sub">{machineCount} machines</div>
      </div>
      <div>
        <span className="compact-cluster-mobile-label">CPU</span>
        <div className="compact-cluster-cell">{formatMetric(live.cpu)}</div>
        <div className="compact-cluster-cell-sub">CPU used / total</div>
      </div>
      <div>
        <span className="compact-cluster-mobile-label">Memory</span>
        <div className="compact-cluster-cell">{formatMetric(live.memory)}</div>
        <div className="compact-cluster-cell-sub">memory used / total</div>
      </div>
      <div
        className="compact-cluster-actions"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => e.stopPropagation()}
      >
        <button type="button" className="secondary btn-icon" title="Edit" onClick={onEdit}>
          <Icon name="edit" size={14} />
        </button>
        <button
          type="button"
          className="secondary btn-icon"
          title="Test"
          onClick={onTest}
          disabled={testing}
        >
          <Icon name="play" size={14} />
        </button>
        <button type="button" className="danger btn-icon" title="Delete" onClick={onRemove}>
          <Icon name="trash" size={14} />
        </button>
      </div>
    </div>
  )
}

export default function Providers() {
  const nav = useNavigate()
  const confirm = useConfirm()
  const [list, setList] = useState([])
  const [clusters, setClusters] = useState([])
  const [metrics, setMetrics] = useState({})
  const [loaded, setLoaded] = useState(false)
  const [wizardOpen, setWizardOpen] = useState(false)
  const [wizardMode, setWizardMode] = useState('create')
  const [editing, setEditing] = useState(null)
  const [error, setError] = useState('')
  const [msg, setMsg] = useState('')
  const [testing, setTesting] = useState(false)

  const load = useCallback(() => {
    Promise.all([
      api('/providers'),
      api('/clusters').catch(() => []),
      api('/dashboard/providers').catch(() => []),
    ])
      .then(([rows, cls, res]) => {
        setError('')
        setList(Array.isArray(rows) ? rows : [])
        setClusters(Array.isArray(cls) ? cls : [])
        const map = {}
        for (const r of Array.isArray(res) ? res : []) {
          if (r?.provider_id) map[r.provider_id] = r
        }
        setMetrics(map)
        setLoaded(true)
      })
      .catch((e) => {
        setError(e.message)
        setLoaded(true)
      })
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
      load()
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

  const online = list.filter((p) => p.availability === 'online').length
  const totalMachines = clusters.reduce(
    (n, c) => n + (Number(c.controlplanes) || 0) + (Number(c.workers) || 0),
    0,
  )

  return (
    <div className="dash-page">
      <PageHeader
        title="Providers"
        description="Manage infrastructure providers used to provision clusters."
        actions={
          <button type="button" className="btn btn-icon" onClick={startCreate}>
            <Icon name="plus" size={16} /> Connect provider
          </button>
        }
      />
      {error && <div className="error">{error}</div>}
      {msg && <p className="muted">{msg}</p>}

      <section className="stat-grid stat-grid-3">
        <StatCard label="Providers" value={loaded ? list.length : '—'} />
        <StatCard
          label="Connected"
          value={loaded ? online : '—'}
          hintTone={online > 0 ? 'ok' : undefined}
        />
        <StatCard label="Clusters" value={loaded ? clusters.length : '—'} hint={`${totalMachines} machines`} />
      </section>

      {list.length === 0 ? (
        <div className="card dash-empty">
          <p className="muted" style={{ margin: 0 }}>
            {loaded ? 'No providers configured.' : 'Loading providers…'}
          </p>
        </div>
      ) : (
        <section className="dash-section">
          <div className="section-toolbar">
            <div>
              <h2
                className="section-kicker"
                style={{ textTransform: 'none', letterSpacing: '-0.01em', fontSize: '0.875rem' }}
              >
                All providers
              </h2>
              <p className="muted dash-section-sub" style={{ margin: '0.25rem 0 0' }}>
                Hypervisors and bare-metal backends
              </p>
            </div>
          </div>
          <div className="compact-cluster-table with-actions">
            <div className="compact-cluster-head">
              <span>Provider</span>
              <span>Status</span>
              <span>Clusters</span>
              <span>CPU</span>
              <span>Memory</span>
              <span />
            </div>
            <div className="compact-cluster-body">
              {list.map((p) => {
                const live = metrics[p.id] || {}
                const providerClusters = clusters.filter((c) => c.provider_id === p.id)
                const clusterCount = providerClusters.length
                const machineCount = providerClusters.reduce(
                  (n, c) => n + (Number(c.controlplanes) || 0) + (Number(c.workers) || 0),
                  0,
                )
                return (
                  <CompactProviderRow
                    key={p.id}
                    provider={p}
                    live={live}
                    clusterCount={clusterCount}
                    machineCount={machineCount}
                    onOpen={() => nav(`/providers/${p.id}`)}
                    onEdit={() => startEdit(p)}
                    onTest={() => testSaved(p.id)}
                    onRemove={() => remove(p.id, p.name)}
                    testing={testing}
                  />
                )
              })}
            </div>
          </div>
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
