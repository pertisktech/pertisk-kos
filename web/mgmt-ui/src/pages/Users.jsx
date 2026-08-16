import { useCallback, useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { api } from '../api'
import { Icon } from '../components/Icons'
import { useConfirm } from '../components/Confirm'
import Checkbox from '../components/Checkbox'

const ROLES = ['viewer', 'operator', 'admin']

export default function Users() {
  const nav = useNavigate()
  const confirm = useConfirm()
  const [list, setList] = useState([])
  const [error, setError] = useState('')
  const [msg, setMsg] = useState('')
  const [showCreate, setShowCreate] = useState(false)
  const [creating, setCreating] = useState(false)
  const [form, setForm] = useState({
    username: '',
    email: '',
    role: 'viewer',
    password: '',
    send_reset_email: false,
  })

  useEffect(() => {
    api('/auth/me')
      .then((u) => {
        if (u?.role !== 'admin') nav('/')
      })
      .catch(() => nav('/login'))
  }, [nav])

  const load = useCallback(() => {
    api('/users')
      .then((data) => setList(Array.isArray(data) ? data : []))
      .catch((e) => setError(e.message || 'failed to load users'))
  }, [])

  useEffect(() => {
    load()
  }, [load])

  function openCreate() {
    setError('')
    setMsg('')
    setForm({
      username: '',
      email: '',
      role: 'viewer',
      password: '',
      send_reset_email: false,
    })
    setShowCreate(true)
  }

  async function onCreate(e) {
    e.preventDefault()
    setCreating(true)
    setError('')
    setMsg('')
    try {
      const body = {
        username: form.username.trim(),
        email: form.email.trim() || null,
        role: form.role,
        send_reset_email: form.send_reset_email,
      }
      if (!form.send_reset_email) {
        body.password = form.password
      }
      await api('/users', { method: 'POST', body })
      setShowCreate(false)
      setMsg(form.send_reset_email ? 'User created; reset email queued.' : 'User created.')
      load()
    } catch (err) {
      setError(err.message)
    } finally {
      setCreating(false)
    }
  }

  async function setRole(u, role) {
    if (role === u.role) return
    setError('')
    try {
      await api(`/users/${u.id}`, { method: 'PATCH', body: { role } })
      load()
    } catch (err) {
      setError(err.message)
    }
  }

  async function toggleDisabled(u) {
    const next = !u.disabled
    const ok = await confirm({
      title: next ? 'Disable user' : 'Enable user',
      message: next
        ? `Disable “${u.username}”? They will not be able to sign in.`
        : `Re-enable “${u.username}”?`,
      confirmLabel: next ? 'Disable' : 'Enable',
      tone: next ? 'danger' : 'primary',
    })
    if (!ok) return
    setError('')
    try {
      await api(`/users/${u.id}`, { method: 'PATCH', body: { disabled: next } })
      load()
    } catch (err) {
      setError(err.message)
    }
  }

  async function sendReset(u) {
    const ok = await confirm({
      title: 'Send password reset',
      message: `Email a password reset link to ${u.email || u.username}?`,
      confirmLabel: 'Send',
      tone: 'primary',
    })
    if (!ok) return
    setError('')
    setMsg('')
    try {
      await api(`/users/${u.id}/reset-password`, { method: 'POST', body: {} })
      setMsg(`Reset email queued for ${u.username}.`)
    } catch (err) {
      setError(err.message)
    }
  }

  return (
    <div>
      <div className="page-head">
        <h1>
          <Icon name="users" size={22} /> Users
        </h1>
        <button type="button" className="btn-icon" onClick={openCreate}>
          <Icon name="plus" size={16} /> Create user
        </button>
      </div>
      {error && <div className="error">{error}</div>}
      {msg && <p className="muted">{msg}</p>}

      {showCreate && (
        <div className="modal-backdrop" role="presentation" onClick={() => setShowCreate(false)}>
          <div
            className="modal-card"
            role="dialog"
            aria-modal="true"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="modal-head">
              <h2>Create local user</h2>
              <button
                type="button"
                className="modal-close secondary btn-icon"
                onClick={() => setShowCreate(false)}
                aria-label="Close"
              >
                <Icon name="x" size={16} />
              </button>
            </div>
            <form onSubmit={onCreate}>
              <div className="field">
                <label htmlFor="u-username">Username</label>
                <input
                  id="u-username"
                  value={form.username}
                  onChange={(e) => setForm({ ...form, username: e.target.value })}
                  required
                  autoComplete="off"
                />
              </div>
              <div className="field">
                <label htmlFor="u-email">Email</label>
                <input
                  id="u-email"
                  type="email"
                  value={form.email}
                  onChange={(e) => setForm({ ...form, email: e.target.value })}
                  required={form.send_reset_email}
                />
              </div>
              <div className="field">
                <label htmlFor="u-role">Role</label>
                <select
                  id="u-role"
                  value={form.role}
                  onChange={(e) => setForm({ ...form, role: e.target.value })}
                >
                  {ROLES.map((r) => (
                    <option key={r} value={r}>
                      {r}
                    </option>
                  ))}
                </select>
              </div>
              <div className="field">
                <Checkbox
                  id="u-reset"
                  checked={form.send_reset_email}
                  onChange={(v) => setForm({ ...form, send_reset_email: v })}
                  label="Send password reset email instead of setting a password"
                />
              </div>
              {!form.send_reset_email && (
                <div className="field">
                  <label htmlFor="u-password">Temporary password</label>
                  <input
                    id="u-password"
                    type="password"
                    value={form.password}
                    onChange={(e) => setForm({ ...form, password: e.target.value })}
                    minLength={8}
                    required
                    autoComplete="new-password"
                  />
                </div>
              )}
              <div className="form-footer">
                <button type="button" className="secondary" onClick={() => setShowCreate(false)}>
                  Cancel
                </button>
                <button type="submit" disabled={creating}>
                  {creating ? 'Creating…' : 'Create'}
                </button>
              </div>
            </form>
          </div>
        </div>
      )}

      <div className="card">
        <table>
          <thead>
            <tr>
              <th>Username</th>
              <th>Email</th>
              <th>Source</th>
              <th>Role</th>
              <th>Status</th>
              <th>Created</th>
              <th>Updated</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {list.map((u) => (
              <tr key={u.id}>
                <td>{u.username}</td>
                <td className="muted">{u.email || '—'}</td>
                <td>
                  <span className="badge">{u.source}</span>
                </td>
                <td>
                  <select
                    value={u.role}
                    onChange={(e) => setRole(u, e.target.value)}
                    aria-label={`Role for ${u.username}`}
                  >
                    {ROLES.map((r) => (
                      <option key={r} value={r}>
                        {r}
                      </option>
                    ))}
                  </select>
                </td>
                <td>
                  <span className={`badge ${u.disabled ? 'error' : 'ready'}`}>
                    {u.disabled ? 'disabled' : 'enabled'}
                  </span>
                </td>
                <td className="mono-inline" style={{ whiteSpace: 'nowrap' }}>
                  {u.created_at}
                </td>
                <td className="mono-inline" style={{ whiteSpace: 'nowrap' }}>
                  {u.updated_at || '—'}
                </td>
                <td>
                  <div className="row-actions">
                    <button
                      type="button"
                      className="secondary btn-icon"
                      title={u.disabled ? 'Enable' : 'Disable'}
                      onClick={() => toggleDisabled(u)}
                    >
                      <Icon name={u.disabled ? 'check' : 'x'} size={14} />
                    </button>
                    {u.source !== 'auth0' && (
                      <button
                        type="button"
                        className="secondary btn-icon"
                        title="Send reset email"
                        disabled={!u.email}
                        onClick={() => sendReset(u)}
                      >
                        <Icon name="mail" size={14} />
                      </button>
                    )}
                  </div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        {list.length === 0 && <p className="muted">No users.</p>}
      </div>
    </div>
  )
}
