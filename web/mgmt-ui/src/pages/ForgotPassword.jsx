import { useEffect, useState } from 'react'
import { Link, useNavigate, useSearchParams } from 'react-router-dom'
import { api } from '../api'
import { Icon } from '../components/Icons'
import { APP_VERSION } from '../utils/version'

export default function ForgotPassword() {
  const nav = useNavigate()
  const [identifier, setIdentifier] = useState('')
  const [error, setError] = useState('')
  const [done, setDone] = useState(false)
  const [loading, setLoading] = useState(false)
  const [localEnabled, setLocalEnabled] = useState(true)

  useEffect(() => {
    api('/auth/mode')
      .then((m) => {
        if (m?.local === false) {
          setLocalEnabled(false)
          nav('/login')
        }
      })
      .catch(() => {})
  }, [nav])

  async function onSubmit(e) {
    e.preventDefault()
    setError('')
    setLoading(true)
    try {
      await api('/auth/password-reset/request', {
        method: 'POST',
        body: { identifier },
      })
      setDone(true)
    } catch (err) {
      setError(err.message)
    } finally {
      setLoading(false)
    }
  }

  if (!localEnabled) return null

  return (
    <div className="login-wrap">
      <div className="login-card">
        <div className="login-brand">
          <span className="login-brand-mark" aria-hidden>
            <Icon name="clusters" size={18} />
          </span>
          <h1>Forgot password</h1>
        </div>
        {error && <div className="error">{error}</div>}
        {done ? (
          <>
            <p>
              If an account matches that username or email, a reset link has been sent when SMTP
              is configured.
            </p>
            <Link to="/login" className="login-submit" style={{ display: 'inline-block', textAlign: 'center' }}>
              Back to sign in
            </Link>
          </>
        ) : (
          <form onSubmit={onSubmit}>
            <div className="field">
              <label htmlFor="fp-ident">Username or email</label>
              <input
                id="fp-ident"
                value={identifier}
                onChange={(e) => setIdentifier(e.target.value)}
                autoComplete="username"
                required
              />
            </div>
            <button type="submit" className="login-submit" disabled={loading}>
              {loading ? 'Sending…' : 'Send reset link'}
            </button>
            <p className="muted" style={{ marginTop: '1rem', marginBottom: 0 }}>
              <Link to="/login">Back to sign in</Link>
            </p>
          </form>
        )}
      </div>
      <footer className="login-footer">
        <p className="version">Pertisk KOS v{APP_VERSION}</p>
      </footer>
    </div>
  )
}

export function ResetPassword() {
  const nav = useNavigate()
  const [params] = useSearchParams()
  const tokenFromUrl = params.get('token') || ''
  const [token, setToken] = useState(tokenFromUrl)
  const [password, setPassword] = useState('')
  const [confirm, setConfirm] = useState('')
  const [error, setError] = useState('')
  const [done, setDone] = useState(false)
  const [loading, setLoading] = useState(false)

  useEffect(() => {
    if (tokenFromUrl) setToken(tokenFromUrl)
  }, [tokenFromUrl])

  async function onSubmit(e) {
    e.preventDefault()
    setError('')
    if (password.length < 8) {
      setError('Password must be at least 8 characters')
      return
    }
    if (password !== confirm) {
      setError('Passwords do not match')
      return
    }
    setLoading(true)
    try {
      await api('/auth/password-reset/confirm', {
        method: 'POST',
        body: { token: token.trim(), password },
      })
      setDone(true)
    } catch (err) {
      setError(err.message)
    } finally {
      setLoading(false)
    }
  }

  return (
    <div className="login-wrap">
      <div className="login-card">
        <div className="login-brand">
          <span className="login-brand-mark" aria-hidden>
            <Icon name="clusters" size={18} />
          </span>
          <h1>Reset password</h1>
        </div>
        {error && <div className="error">{error}</div>}
        {done ? (
          <>
            <p>Your password has been updated. You can sign in now.</p>
            <button type="button" className="login-submit" onClick={() => nav('/login')}>
              Sign in
            </button>
          </>
        ) : (
          <form onSubmit={onSubmit}>
            {!tokenFromUrl && (
              <div className="field">
                <label htmlFor="rp-token">Reset token</label>
                <input
                  id="rp-token"
                  value={token}
                  onChange={(e) => setToken(e.target.value)}
                  required
                />
              </div>
            )}
            <div className="field">
              <label htmlFor="rp-password">New password</label>
              <input
                id="rp-password"
                type="password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                minLength={8}
                required
                autoComplete="new-password"
              />
            </div>
            <div className="field">
              <label htmlFor="rp-confirm">Confirm password</label>
              <input
                id="rp-confirm"
                type="password"
                value={confirm}
                onChange={(e) => setConfirm(e.target.value)}
                minLength={8}
                required
                autoComplete="new-password"
              />
            </div>
            <button type="submit" className="login-submit" disabled={loading || !token.trim()}>
              {loading ? 'Saving…' : 'Set password'}
            </button>
            <p className="muted" style={{ marginTop: '1rem', marginBottom: 0 }}>
              <Link to="/login">Back to sign in</Link>
            </p>
          </form>
        )}
      </div>
      <footer className="login-footer">
        <p className="version">Pertisk KOS v{APP_VERSION}</p>
      </footer>
    </div>
  )
}
