import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { api, setToken, getToken } from '../api'

export default function Login() {
  const nav = useNavigate()
  const [username, setUsername] = useState('admin')
  const [password, setPassword] = useState('')
  const [mode, setMode] = useState(null)
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(false)

  useEffect(() => {
    // Auth0 callback: /#/auth/callback?token=...
    const hash = window.location.hash
    if (hash.includes('auth/callback')) {
      const q = new URLSearchParams(hash.split('?')[1] || '')
      const token = q.get('token')
      if (token) {
        setToken(token)
        nav('/')
        return
      }
    }
    if (getToken()) nav('/')
    api('/auth/mode').then(setMode).catch(() => setMode({ local: true, auth0: false }))
  }, [nav])

  async function onSubmit(e) {
    e.preventDefault()
    setError('')
    setLoading(true)
    try {
      const res = await api('/auth/login', {
        method: 'POST',
        body: { username, password },
      })
      setToken(res.token)
      nav('/')
    } catch (err) {
      setError(err.message)
    } finally {
      setLoading(false)
    }
  }

  return (
    <div className="login-wrap">
      <div className="login-card">
        <h1>Pertisk Management</h1>
        <p>Sign in to manage Proxmox-backed clusters (HA CP + workers).</p>
        {error && <div className="error">{error}</div>}
        {mode?.local !== false && (
          <form onSubmit={onSubmit}>
            <div className="field">
              <label>Username</label>
              <input value={username} onChange={(e) => setUsername(e.target.value)} autoComplete="username" />
            </div>
            <div className="field">
              <label>Password</label>
              <input type="password" value={password} onChange={(e) => setPassword(e.target.value)} autoComplete="current-password" />
            </div>
            <button type="submit" disabled={loading} style={{ width: '100%' }}>
              {loading ? 'Signing in…' : 'Sign in'}
            </button>
          </form>
        )}
        {mode?.auth0 && (
          <p style={{ marginTop: '1rem' }}>
            <a href="/api/auth/oidc/start">Continue with Auth0 (SSO)</a>
          </p>
        )}
      </div>
    </div>
  )
}
