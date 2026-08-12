import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import {
  api,
  setToken,
  getToken,
  getRememberMe,
  setRememberMe,
  getSavedCredentials,
  saveCredentials,
  clearSavedCredentials,
  setAuthProvider,
} from '../api'
import Checkbox from '../components/Checkbox'
import { APP_VERSION } from '../utils/version'

function Auth0Mark({ size = 18 }) {
  return (
    <svg
      className="login-sso-mark"
      width={size}
      height={size}
      viewBox="0 0 24 24"
      aria-hidden
    >
      <path
        fill="currentColor"
        d="M21.98 7.448L19.62 0H4.355L2.02 7.448c-1.352 4.233.198 8.083 3.515 10.397l6.48 4.697 6.451-4.697c3.315-2.314 4.866-6.164 3.513-10.397zM12.016 17.76l-2.297-7.1h4.593l-2.296 7.1zm-2.76-8.386L6.391 1.71h11.218l-2.865 7.664H9.256z"
      />
    </svg>
  )
}

export default function Login() {
  const nav = useNavigate()
  const saved = getSavedCredentials()
  const [username, setUsername] = useState(saved?.username || '')
  const [password, setPassword] = useState(saved?.password || '')
  const [remember, setRemember] = useState(() => getRememberMe() || !!saved?.username)
  const [mode, setMode] = useState(null)
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(false)

  const showLocal = mode?.local !== false
  const showAuth0 = !!mode?.auth0

  useEffect(() => {
    // Auth0 callback: /#/auth/callback?token=...
    const hash = window.location.hash
    if (hash.includes('auth/callback')) {
      const q = new URLSearchParams(hash.split('?')[1] || '')
      const token = q.get('token')
      if (token) {
        setAuthProvider('auth0')
        setToken(token, getRememberMe())
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
      setRememberMe(remember)
      if (remember) {
        saveCredentials(username, password)
      } else {
        clearSavedCredentials()
      }
      setAuthProvider('local')
      setToken(res.token, remember)
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
        <div className="login-brand">
          <span className="login-brand-mark" aria-hidden>P</span>
          <h1>Pertisk KOS</h1>
        </div>
        {error && <div className="error">{error}</div>}

        {showAuth0 && !showLocal && (
          <a className="login-sso-btn login-sso-primary" href="/api/auth/oidc/start">
            <Auth0Mark />
            <span>Continue with Auth0</span>
          </a>
        )}

        {showLocal && (
          <form onSubmit={onSubmit}>
            <div className="field">
              <label htmlFor="login-username">Username</label>
              <input
                id="login-username"
                value={username}
                onChange={(e) => setUsername(e.target.value)}
                autoComplete="username"
              />
            </div>
            <div className="field">
              <label htmlFor="login-password">Password</label>
              <input
                id="login-password"
                type="password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                autoComplete="current-password"
              />
            </div>
            <div className="field login-remember">
              <Checkbox
                id="login-remember"
                checked={remember}
                onChange={setRemember}
                label="Remember password"
              />
            </div>
            <button type="submit" className="login-submit" disabled={loading}>
              {loading ? 'Signing in…' : 'Sign in'}
            </button>
          </form>
        )}

        {showLocal && showAuth0 && (
          <>
            <div className="login-divider" role="separator" aria-label="or">
              <span>or</span>
            </div>
            <a className="login-sso-btn" href="/api/auth/oidc/start">
              <Auth0Mark />
              <span>Continue with Auth0 SSO</span>
            </a>
          </>
        )}
      </div>
      <footer className="login-footer">
        <p className="version">Pertisk KOS v{APP_VERSION}</p>
      </footer>
    </div>
  )
}
