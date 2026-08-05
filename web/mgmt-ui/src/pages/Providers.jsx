import { useEffect, useState } from 'react'
import { api } from '../api'
import { Icon } from '../components/Icons'
import { useConfirm } from '../components/Confirm'

const emptyForm = {
  name: 'lab-pve',
  url: 'https://10.1.1.197:8006',
  token_id: '',
  token_secret: '',
  node: 'pve',
  storage: 'local-lvm',
  bridge: 'vmbr0',
  insecure: true,
}

function formatProbe(r) {
  const nodes = (r.nodes || []).map((n) => n.node).join(', ') || '(none)'
  const parts = [
    `Proxmox ${r.version || '?'} @ ${r.url}`,
    `nodes: ${nodes}`,
    r.node_ok ? `node OK (${r.node_message || 'ok'})` : `node FAIL: ${r.node_message || 'unknown'}`,
  ]
  if (r.storage) {
    parts.push(
      r.storage.ok
        ? `storage OK: ${r.storage.storage} (${r.storage.type_ || r.storage.type || '?'})`
        : `storage FAIL: ${r.storage.message}`,
    )
  }
  return parts.join(' — ')
}

export default function Providers() {
  const confirm = useConfirm()
  const [list, setList] = useState([])
  const [editingId, setEditingId] = useState(null)
  const [form, setForm] = useState({ ...emptyForm })
  const [storageOptions, setStorageOptions] = useState([])
  const [error, setError] = useState('')
  const [msg, setMsg] = useState('')
  const [saving, setSaving] = useState(false)
  const [testing, setTesting] = useState(false)

  function load() {
    api('/providers').then(setList).catch((e) => setError(e.message))
  }
  useEffect(load, [])

  function set(k, v) {
    setForm((f) => ({ ...f, [k]: v }))
  }

  function startCreate() {
    setError('')
    setMsg('')
    setStorageOptions([])
    setEditingId('new')
    setForm({ ...emptyForm })
  }

  function startEdit(p) {
    setError('')
    setMsg('')
    setStorageOptions(p.storage ? [p.storage] : [])
    setEditingId(p.id)
    setForm({
      name: p.name,
      url: p.url,
      token_id: p.token_id,
      token_secret: '',
      node: p.node,
      storage: p.storage,
      bridge: p.bridge || 'vmbr0',
      insecure: !!p.insecure,
    })
  }

  function cancelForm() {
    setEditingId(null)
    setStorageOptions([])
    setForm({ ...emptyForm })
  }

  function applyProbeResult(r) {
    const available = r.storage?.available || []
    setForm((f) => {
      if (available.length && f.storage && !available.includes(f.storage)) {
        return { ...f, storage: available[0] }
      }
      return f
    })
    if (available.length) {
      setStorageOptions(available)
    } else if (r.storage?.storage) {
      setStorageOptions([r.storage.storage])
    }
    const text = formatProbe(r)
    if (r.ok) {
      setMsg(`OK — ${text}`)
      setError('')
    } else {
      setMsg('')
      setError(text)
    }
  }

  async function save(e) {
    e.preventDefault()
    setError('')
    setMsg('')
    setSaving(true)
    try {
      if (editingId === 'new') {
        if (!form.token_secret) throw new Error('Token secret is required')
        await api('/providers', { method: 'POST', body: form })
        setMsg('Provider created')
      } else {
        const body = {
          name: form.name,
          url: form.url,
          token_id: form.token_id,
          node: form.node,
          storage: form.storage,
          bridge: form.bridge,
          insecure: form.insecure,
        }
        if (form.token_secret) body.token_secret = form.token_secret
        await api(`/providers/${editingId}`, { method: 'PUT', body })
        setMsg('Provider updated')
      }
      setEditingId(null)
      setStorageOptions([])
      load()
    } catch (err) {
      setError(err.message)
    } finally {
      setSaving(false)
    }
  }

  async function testSaved(id) {
    setError('')
    setMsg('Testing…')
    setTesting(true)
    try {
      const r = await api(`/providers/${id}/test`, { method: 'POST', body: {} })
      applyProbeResult(r)
    } catch (err) {
      setMsg('')
      setError(err.message)
    } finally {
      setTesting(false)
    }
  }

  async function testDraft() {
    setError('')
    setMsg('Testing…')
    setTesting(true)
    try {
      if (editingId === 'new' || form.token_secret) {
        if (!form.token_secret) throw new Error('Token secret is required to test')
        const r = await api('/providers/probe', {
          method: 'POST',
          body: {
            url: form.url,
            token_id: form.token_id,
            token_secret: form.token_secret,
            node: form.node,
            storage: form.storage,
            insecure: form.insecure,
          },
        })
        applyProbeResult(r)
      } else {
        const r = await api(`/providers/${editingId}/test`, {
          method: 'POST',
          body: { node: form.node, storage: form.storage },
        })
        applyProbeResult(r)
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
      if (editingId === id) cancelForm()
      load()
    } catch (err) {
      setError(err.message)
    }
  }

  const formTitle = editingId === 'new' ? 'New Proxmox provider' : 'Edit Proxmox provider'
  const storageList = storageOptions.length
    ? storageOptions
    : form.storage
      ? [form.storage]
      : []

  return (
    <div>
      <div className="page-head">
        <h1><Icon name="providers" size={22} /> Providers</h1>
        {editingId ? (
          <button type="button" className="secondary btn-icon" onClick={cancelForm}>
            <Icon name="x" size={16} /> Cancel
          </button>
        ) : (
          <button type="button" className="btn-icon" onClick={startCreate}>
            <Icon name="plus" size={16} /> Add Proxmox
          </button>
        )}
      </div>
      {error && <div className="error">{error}</div>}
      {msg && <p className="muted">{msg}</p>}
      {editingId && (
        <form className="card" onSubmit={save}>
          <h2 className="card-title"><Icon name="edit" size={18} /> {formTitle}</h2>
          <p className="muted">
            Lab Proxmox uses a self-signed cert — keep <strong>Insecure TLS = Yes</strong>.
            Leave token secret blank when editing to keep the existing secret.
            Use <strong>Test</strong> to validate connection, node, and storage before saving.
          </p>
          <div className="form-grid">
            <div className="field"><label>Name</label><input value={form.name} onChange={(e) => set('name', e.target.value)} required /></div>
            <div className="field"><label>URL</label><input value={form.url} onChange={(e) => set('url', e.target.value)} placeholder="https://10.1.1.197:8006" required /></div>
            <div className="field"><label>Token ID</label><input value={form.token_id} onChange={(e) => set('token_id', e.target.value)} placeholder="root@pam!pertisk" required /></div>
            <div className="field">
              <label>Token secret{editingId !== 'new' ? ' (leave blank to keep)' : ''}</label>
              <input
                type="password"
                value={form.token_secret}
                onChange={(e) => set('token_secret', e.target.value)}
                required={editingId === 'new'}
                placeholder={editingId !== 'new' ? '••••••••' : ''}
                autoComplete="new-password"
              />
            </div>
            <div className="field"><label>Node</label><input value={form.node} onChange={(e) => set('node', e.target.value)} required /></div>
            <div className="field">
              <label>Storage</label>
              {storageList.length > 1 ? (
                <select value={form.storage} onChange={(e) => set('storage', e.target.value)} required>
                  {storageList.map((s) => (
                    <option key={s} value={s}>{s}</option>
                  ))}
                </select>
              ) : (
                <input
                  value={form.storage}
                  onChange={(e) => set('storage', e.target.value)}
                  list="provider-storage-options"
                  required
                />
              )}
              <datalist id="provider-storage-options">
                {storageList.map((s) => (
                  <option key={s} value={s} />
                ))}
              </datalist>
              <p className="hint muted">Run Test to discover storages that support images on this node.</p>
            </div>
            <div className="field"><label>Bridge</label><input value={form.bridge} onChange={(e) => set('bridge', e.target.value)} /></div>
            <div className="field">
              <label>Insecure TLS</label>
              <select value={form.insecure ? '1' : '0'} onChange={(e) => set('insecure', e.target.value === '1')}>
                <option value="1">Yes (lab / self-signed)</option>
                <option value="0">No</option>
              </select>
            </div>
          </div>
          <div className="form-footer" style={{ display: 'flex', gap: '0.75rem', marginTop: '1rem' }}>
            <button type="button" className="secondary btn-icon" onClick={testDraft} disabled={testing || saving}>
              <Icon name="play" size={16} /> {testing ? 'Testing…' : 'Test'}
            </button>
            <button type="submit" className="btn-icon" disabled={saving || testing}>
              <Icon name="check" size={16} /> {saving ? 'Saving…' : 'Save'}
            </button>
          </div>
        </form>
      )}
      <div className="card">
        <table>
          <thead>
            <tr><th>Name</th><th>URL</th><th>Node</th><th>Storage</th><th>TLS</th><th></th></tr>
          </thead>
          <tbody>
            {list.map((p) => (
              <tr key={p.id}>
                <td>{p.name}</td>
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
    </div>
  )
}
