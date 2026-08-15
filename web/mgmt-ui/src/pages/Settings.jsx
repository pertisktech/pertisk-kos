import { useEffect, useState } from 'react'
import { api } from '../api'
import { Icon } from '../components/Icons'
import { APP_VERSION } from '../utils/version'

function PathRow({ label, info }) {
  if (!info) return null
  return (
    <div>
      <dt>{label}</dt>
      <dd>
        <code className="mono-inline">{info.path}</code>
        {' '}
        <span className={`badge ${info.exists ? 'ready' : 'error'}`}>
          {info.exists ? 'found' : 'missing'}
        </span>
      </dd>
    </div>
  )
}

function BoolBadge({ on, onLabel = 'configured', offLabel = 'not set' }) {
  return (
    <span className={`badge ${on ? 'ready' : ''}`}>
      {on ? onLabel : offLabel}
    </span>
  )
}

function formatTtl(secs) {
  if (secs == null) return '—'
  if (secs % 86400 === 0) return `${secs / 86400}d (${secs}s)`
  if (secs % 3600 === 0) return `${secs / 3600}h (${secs}s)`
  return `${secs}s`
}

export default function Settings() {
  const [cfg, setCfg] = useState(null)
  const [me, setMe] = useState(null)
  const [error, setError] = useState('')

  useEffect(() => {
    Promise.all([
      api('/settings').catch((e) => {
        throw e
      }),
      api('/auth/me').catch(() => null),
    ])
      .then(([s, m]) => {
        setCfg(s)
        setMe(m)
        setError('')
      })
      .catch((e) => setError(e.message || 'failed to load settings'))
  }, [])

  return (
    <div>
      <div className="page-head">
        <h1><Icon name="settings" size={22} /> Settings</h1>
      </div>
      {error && <div className="error">{error}</div>}

      <div className="card">
        <h2 className="card-title"><Icon name="user" size={18} /> Session</h2>
        {me ? (
          <dl className="kv">
            <div>
              <dt>User</dt>
              <dd>{me.username}</dd>
            </div>
            <div>
              <dt>Role</dt>
              <dd><span className="badge ready">{me.role}</span></dd>
            </div>
          </dl>
        ) : (
          <p className="muted">Loading session…</p>
        )}
      </div>

      <div className="card">
        <h2 className="card-title"><Icon name="dashboard" size={18} /> Service</h2>
        {!cfg ? (
          <p className="muted">Loading service config…</p>
        ) : (
          <dl className="kv settings-kv">
            <div>
              <dt>Service</dt>
              <dd>{cfg.service}</dd>
            </div>
            <div>
              <dt>Version</dt>
              <dd>
                <span className="mono-inline">v{cfg.version}</span>
                {APP_VERSION !== cfg.version && (
                  <span className="muted" style={{ marginLeft: 8 }}>
                    UI v{APP_VERSION}
                  </span>
                )}
              </dd>
            </div>
            <div>
              <dt>Listen</dt>
              <dd><code className="mono-inline">{cfg.listen}</code></dd>
            </div>
            <div>
              <dt>Public URL</dt>
              <dd><code className="mono-inline">{cfg.public_url}</code></dd>
              <p className="hint muted" style={{ marginTop: 6 }}>
                From <code>MGMT_PUBLIC_URL</code> (required when listen is{' '}
                <code>0.0.0.0</code>). Applied on cluster create as{' '}
                <code>machine.dashboard.mgmt_url</code> (serial console).
                Example: <code>https://mgmt.example.com</code>.
              </p>
            </div>
            <div>
              <dt>JWT TTL</dt>
              <dd>{formatTtl(cfg.jwt_ttl_secs)}</dd>
            </div>
            <div>
              <dt>Metrics token</dt>
              <dd>
                <BoolBadge on={cfg.metrics_token_configured} />
              </dd>
            </div>
          </dl>
        )}
      </div>

      <div className="card">
        <h2 className="card-title"><Icon name="folder" size={18} /> Paths</h2>
        {!cfg ? (
          <p className="muted">Loading…</p>
        ) : (
          <dl className="kv settings-kv">
            <PathRow label="Database" info={cfg.db} />
            <PathRow label="Data dir" info={cfg.data_dir} />
            <PathRow label="Jobs" info={cfg.jobs_dir} />
            <PathRow label="Kubeconfigs" info={cfg.kubeconfigs_dir} />
            <PathRow label="Images" info={cfg.images_dir} />
            <PathRow label="lab-up" info={cfg.lab_up} />
            <PathRow label="pertiskctl" info={cfg.pertiskctl} />
          </dl>
        )}
      </div>

      <div className="card">
        <h2 className="card-title"><Icon name="shield" size={18} /> Authentication</h2>
        {!cfg ? (
          <p className="muted">Loading…</p>
        ) : (
          <>
            <dl className="kv settings-kv">
              <div>
                <dt>Mode</dt>
                <dd><span className="badge">{cfg.auth.mode}</span></dd>
              </div>
              <div>
                <dt>Local</dt>
                <dd>
                  <BoolBadge
                    on={cfg.auth.local}
                    onLabel="enabled"
                    offLabel="off"
                  />
                </dd>
              </div>
              <div>
                <dt>Auth0 SSO</dt>
                <dd>
                  <BoolBadge
                    on={cfg.auth.auth0}
                    onLabel="enabled"
                    offLabel="off"
                  />
                </dd>
              </div>
              <div>
                <dt>Admin user</dt>
                <dd className="mono-inline">{cfg.auth.admin_user}</dd>
              </div>
              <div>
                <dt>Admin password</dt>
                <dd>
                  <BoolBadge on={cfg.auth.admin_password_configured} />
                </dd>
              </div>
              {cfg.auth.auth0 && (
                <>
                  <div>
                    <dt>Auth0 domain</dt>
                    <dd className="mono-inline">{cfg.auth.auth0_domain || '—'}</dd>
                  </div>
                  <div>
                    <dt>Client ID</dt>
                    <dd className="mono-inline">{cfg.auth.auth0_client_id || '—'}</dd>
                  </div>
                  <div>
                    <dt>Audience</dt>
                    <dd className="mono-inline">{cfg.auth.auth0_audience || '—'}</dd>
                  </div>
                </>
              )}
            </dl>
            <p className="muted" style={{ marginTop: '1rem', marginBottom: 0 }}>
              Configure via env: <code className="mono-inline">AUTH_MODE</code>,{' '}
              <code className="mono-inline">AUTH0_*</code>,{' '}
              <code className="mono-inline">MGMT_ADMIN_USER</code>,{' '}
              <code className="mono-inline">MGMT_SECRET_KEY</code>,{' '}
              <code className="mono-inline">MGMT_PUBLIC_URL</code>.
            </p>
          </>
        )}
      </div>
    </div>
  )
}
