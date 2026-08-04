import { useEffect, useState } from 'react'
import { api } from '../api'

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

export default function Providers() {
  const [list, setList] = useState([])
  const [editingId, setEditingId] = useState(null) // null | 'new' | id
  const [form, setForm] = useState({ ...emptyForm })
  const [error, setError] = useState('')
  const [msg, setMsg] = useState('')
  const [saving, setSaving] = useState(false)

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
    setEditingId('new')
    setForm({ ...emptyForm })
  }

  function startEdit(p) {
    setError('')
    setMsg('')
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
    setForm({ ...emptyForm })
  }

  async function save(e) {
    e.preventDefault()
    setError('')
    setMsg('')
    setSaving(true)
    try {
      if (editingId === 'new') {
        if (!form.token_secret) {
          throw new Error('Token secret is required')
        }
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
        if (form.token_secret) {
          body.token_secret = form.token_secret
        }
        await api(`/providers/${editingId}`, { method: 'PUT', body })
        setMsg('Provider updated')
      }
      setEditingId(null)
      load()
    } catch (err) {
      setError(err.message)
    } finally {
      setSaving(false)
    }
  }

  async function test(id) {
    setError('')
    setMsg('Testing…')
    try {
      const r = await api(`/providers/${id}/test`, { method: 'POST' })
      setMsg(
        `OK — Proxmox ${r.version} @ ${r.url} (insecure=${r.insecure}), nodes: ${
          r.nodes.map((n) => n.node).join(', ') || '(none)'
        }`,
      )
    } catch (err) {
      setMsg('')
      setError(err.message)
    }
  }

  async function remove(id) {
    if (!confirm('Delete provider?')) return
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

  return (
    <div>
      <div className="page-head">
        <h1>Providers</h1>
        {editingId ? (
          <button type="button" className="secondary" onClick={cancelForm}>Cancel</button>
        ) : (
          <button type="button" onClick={startCreate}>Add Proxmox</button>
        )}
      </div>
      {error && <div className="error">{error}</div>}
      {msg && <p className="muted">{msg}</p>}
      {editingId && (
        <form className="card" onSubmit={save}>
          <h2>{formTitle}</h2>
          <p className="muted">
            Lab Proxmox uses a self-signed cert — keep <strong>Insecure TLS = Yes</strong>.
            Leave token secret blank when editing to keep the existing secret.
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
            <div className="field"><label>Storage</label><input value={form.storage} onChange={(e) => set('storage', e.target.value)} required /></div>
            <div className="field"><label>Bridge</label><input value={form.bridge} onChange={(e) => set('bridge', e.target.value)} /></div>
            <div className="field">
              <label>Insecure TLS</label>
              <select value={form.insecure ? '1' : '0'} onChange={(e) => set('insecure', e.target.value === '1')}>
                <option value="1">Yes (lab / self-signed)</option>
                <option value="0">No</option>
              </select>
            </div>
          </div>
          <button type="submit" disabled={saving}>{saving ? 'Saving…' : 'Save'}</button>
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
                  <button type="button" className="secondary" onClick={() => startEdit(p)}>Edit</button>
                  <button type="button" className="secondary" onClick={() => test(p.id)}>Test</button>
                  <button type="button" className="danger" onClick={() => remove(p.id)}>Delete</button>
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
