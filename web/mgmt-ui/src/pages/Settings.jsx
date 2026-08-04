import { useEffect, useState } from 'react'
import { api } from '../api'

export default function Settings() {
  const [mode, setMode] = useState(null)
  const [me, setMe] = useState(null)

  useEffect(() => {
    api('/auth/mode').then(setMode)
    api('/auth/me').then(setMe)
  }, [])

  return (
    <div>
      <div className="page-head"><h1>Settings</h1></div>
      <div className="card">
        <h2>Session</h2>
        {me && <p>{me.username} · role <strong>{me.role}</strong></p>}
      </div>
      <div className="card">
        <h2>Authentication</h2>
        {mode && (
          <ul className="muted">
            <li>Mode: {mode.mode}</li>
            <li>Local (SQLite): {mode.local ? 'enabled' : 'off'}</li>
            <li>Auth0 SSO: {mode.auth0 ? `enabled (${mode.auth0_domain})` : 'off'}</li>
          </ul>
        )}
        <p className="muted">Configure via AUTH_MODE, AUTH0_*, MGMT_ADMIN_USER, MGMT_SECRET_KEY.</p>
      </div>
    </div>
  )
}
