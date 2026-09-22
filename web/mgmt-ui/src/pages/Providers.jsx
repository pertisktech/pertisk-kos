import { useCallback, useEffect, useMemo, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { api } from '../api'
import { Icon } from '../components/Icons'
import PageHeader from '../components/PageHeader'
import StatCard from '../components/StatCard'
import { ResourceBars, formatMetric } from '../components/ResourceBars'
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

function availTone(availability) {
  if (availability === 'online') return 'ok'
  if (availability === 'offline') return 'err'
  return 'warn'
}

function availLabel(availability) {
  if (availability === 'online') return 'online'
  if (availability === 'offline') return 'offline'
  if (!availability) return 'unknown'
  return availability
}

function FleetProviderRow({
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
  const meta = [formatProviderKind(provider.kind), provider.node, provider.arch || 'amd64']
    .filter(Boolean)
    .join(' · ')

  return (
    <div className="fleet-row fleet-row-provider fleet-row-actions">
      <button type="button" className="fleet-row-main" onClick={onOpen}>
        <div className="fleet-cell fleet-cell-name">
          <span className={`fleet-mark ${tone}`} aria-hidden>
            {providerKindGlyph(provider.kind)}
          </span>
          <div className="fleet-name-stack">
            <span className="fleet-name">{provider.name}</span>
            <span className="fleet-sub">{provider.url || meta}</span>
          </div>
        </div>
        <span className={`fleet-cell fleet-status ${tone}`}>{availLabel(provider.availability)}</span>
        <span className="fleet-cell">
          {clusterCount} clusters
          <span className="fleet-sub-inline"> · {machineCount} nodes</span>
        </span>
        <span className="fleet-cell fleet-mono" title={formatMetric(live.cpu)}>
          {formatMetric(live.cpu)}
        </span>
        <span className="fleet-cell fleet-mono" title={formatMetric(live.memory)}>
          {formatMetric(live.memory)}
        </span>
        <div className="fleet-cell fleet-cell-bars">
          <ResourceBars cpu={live.cpu} memory={live.memory} disk={live.disk} />
        </div>
      </button>
      <div className="fleet-actions">
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
  const [filter, setFilter] = useState('')
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

  const filtered = useMemo(() => {
    const q = filter.trim().toLowerCase()
    if (!q) return list
    return list.filter((p) => {
      const hay = [p.name, p.kind, p.url, p.node, p.availability].filter(Boolean).join(' ').toLowerCase()
      return hay.includes(q)
    })
  }, [list, filter])

  return (
    <div className="dash-page">
      <PageHeader
        title="Providers"
        description="Hypervisors and backends used to provision clusters."
        actions={
          <button type="button" className="btn btn-icon" onClick={startCreate}>
            <Icon name="plus" size={16} /> Connect provider
          </button>
        }
      />
      {error && <div className="error">{error}</div>}
      {msg && <p className="muted">{msg}</p>}

      <section className="stat-grid stat-grid-3">
        <StatCard icon="providers" label="Providers" value={loaded ? list.length : '—'} />
        <StatCard
          icon="check"
          label="Connected"
          value={loaded ? online : '—'}
          hintTone={online > 0 ? 'ok' : undefined}
        />
        <StatCard
          icon="clusters"
          label="Clusters"
          value={loaded ? clusters.length : '—'}
          hint={`${totalMachines} nodes`}
        />
      </section>

      <section className="fleet-panel">
        <div className="fleet-panel-head">
          <div className="fleet-panel-title-row">
            <h2 className="fleet-panel-title">All providers</h2>
          </div>
          <div className="fleet-filter">
            <Icon name="search" size={14} />
            <input
              type="search"
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
              placeholder="Filter…"
              aria-label="Filter providers"
            />
          </div>
        </div>
        {list.length === 0 ? (
          <div className="fleet-empty">
            <p className="muted" style={{ margin: 0 }}>
              {loaded ? 'No providers configured.' : 'Loading providers…'}
            </p>
          </div>
        ) : filtered.length === 0 ? (
          <div className="fleet-empty muted">No providers match this filter.</div>
        ) : (
          <div className="fleet-scroll">
            <div className="fleet-table fleet-table-providers-actions">
              <div className="fleet-head" aria-hidden>
                <span>Provider</span>
                <span>Status</span>
                <span>Clusters</span>
                <span>CPU</span>
                <span>Memory</span>
                <span>Resources</span>
                <span />
              </div>
              <div className="fleet-body">
                {filtered.map((p) => {
                  const live = metrics[p.id] || {}
                  const providerClusters = clusters.filter((c) => c.provider_id === p.id)
                  const clusterCount = providerClusters.length
                  const machineCount = providerClusters.reduce(
                    (n, c) => n + (Number(c.controlplanes) || 0) + (Number(c.workers) || 0),
                    0,
                  )
                  return (
                    <FleetProviderRow
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
          </div>
        )}
      </section>

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
