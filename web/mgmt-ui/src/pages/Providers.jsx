import { useEffect, useState } from 'react'
import { api } from '../api'
import { Icon } from '../components/Icons'
import { useConfirm } from '../components/Confirm'
import ProviderWizard from '../components/ProviderWizard'

function formatProbe(r, kind) {
  const label = kind === 'vsphere' ? 'ESXi' : 'Proxmox'
  const nodes = (r.nodes || []).map((n) => n.node).join(', ') || '(none)'
  const parts = [
    `${label} ${r.version || '?'} @ ${r.url}`,
    `${kind === 'vsphere' ? 'hosts' : 'nodes'}: ${nodes}`,
    r.node_ok
      ? `${kind === 'vsphere' ? 'host' : 'node'} OK (${r.node_message || 'ok'})`
      : `${kind === 'vsphere' ? 'host' : 'node'} FAIL: ${r.node_message || 'unknown'}`,
  ]
  if (r.arch) {
    parts.push(`guest arch → ${r.arch}`)
  }
  if (r.storage) {
    parts.push(
      r.storage.ok
        ? `${kind === 'vsphere' ? 'datastore' : 'storage'} OK: ${r.storage.storage} (${r.storage.type_ || r.storage.type || '?'})`
        : `${kind === 'vsphere' ? 'datastore' : 'storage'} FAIL: ${r.storage.message}`,
    )
  }
  return parts.join(' — ')
}

export default function Providers() {
  const confirm = useConfirm()
  const [list, setList] = useState([])
  const [wizardOpen, setWizardOpen] = useState(false)
  const [wizardMode, setWizardMode] = useState('create')
  const [editing, setEditing] = useState(null)
  const [error, setError] = useState('')
  const [msg, setMsg] = useState('')
  const [testing, setTesting] = useState(false)

  function load() {
    api('/providers').then(setList).catch((e) => setError(e.message))
  }
  useEffect(load, [])

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
    <div>
      <div className="page-head">
        <h1><Icon name="providers" size={22} /> Providers</h1>
        <button type="button" className="btn-icon" onClick={startCreate}>
          <Icon name="plus" size={16} /> Add provider
        </button>
      </div>
      {error && <div className="error">{error}</div>}
      {msg && <p className="muted">{msg}</p>}
      <div className="card">
        <table>
          <thead>
            <tr>
              <th>Name</th>
              <th>Kind</th>
              <th>Arch</th>
              <th>URL</th>
              <th>Host / Node</th>
              <th>Storage</th>
              <th>TLS</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {list.map((p) => (
              <tr key={p.id}>
                <td>{p.name}</td>
                <td>{p.kind || 'proxmox'}</td>
                <td>{p.arch || 'amd64'}</td>
                <td className="mono">{p.url}</td>
                <td>{p.node}</td>
                <td>{p.storage}</td>
                <td>{p.insecure ? 'insecure' : 'verify'}</td>
                <td className="row-actions">
                  <button type="button" className="secondary btn-icon" onClick={() => startEdit(p)}>
                    <Icon name="edit" size={14} /> Edit
                  </button>
                  <button type="button" className="secondary btn-icon" onClick={() => testSaved(p.id)} disabled={testing}>
                    <Icon name="play" size={14} /> Test
                  </button>
                  <button type="button" className="danger btn-icon" onClick={() => remove(p.id, p.name)}>
                    <Icon name="trash" size={14} />
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        {list.length === 0 && <p className="muted">No providers configured.</p>}
      </div>

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
