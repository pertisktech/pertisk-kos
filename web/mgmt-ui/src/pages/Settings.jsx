import { useEffect, useState } from 'react'
import { useSearchParams } from 'react-router-dom'
import { api } from '../api'
import { Icon } from '../components/Icons'
import { APP_VERSION } from '../utils/version'

const TABS = [
  { id: 'session', label: 'Session', icon: 'user' },
  { id: 'service', label: 'Service', icon: 'dashboard' },
  { id: 'paths', label: 'Paths', icon: 'folder' },
  { id: 'auth', label: 'Authentication', icon: 'shield' },
  { id: 'email', label: 'Email', icon: 'mail' },
]

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

function Loading() {
  return <p className="muted">Loading…</p>
}

export default function Settings() {
  const [search, setSearch] = useSearchParams()
  const [cfg, setCfg] = useState(null)
  const [me, setMe] = useState(null)
  const [error, setError] = useState('')

  const tab = TABS.some((t) => t.id === search.get('tab'))
    ? search.get('tab')
    : 'session'

  function setTab(next) {
    setSearch(next === 'session' ? {} : { tab: next }, { replace: true })
  }

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
        <h1>
          <Icon name="settings" size={22} /> Settings
        </h1>
      </div>
      {error && <div className="error">{error}</div>}

      <div className="tabs-shell">
        <div className="tabs" role="tablist" aria-label="Settings sections">
          {TABS.map((t) => (
            <button
              key={t.id}
              type="button"
              role="tab"
              aria-selected={tab === t.id}
              className={`tab ${tab === t.id ? 'active' : ''}`}
              onClick={() => setTab(t.id)}
            >
              <Icon name={t.icon} size={16} />
              {t.label}
            </button>
          ))}
        </div>

        <div className="tab-panel card" role="tabpanel">
          {tab === 'session' && (
            <div className="tab-body">
              <p className="section-label">Signed-in account</p>
              {me ? (
                <dl className="kv settings-kv">
                  <div>
                    <dt>User</dt>
                    <dd>{me.username}</dd>
                  </div>
                  <div>
                    <dt>Role</dt>
                    <dd>
                      <span className="badge ready">{me.role}</span>
                    </dd>
                  </div>
                  {me.provider && (
                    <div>
                      <dt>Provider</dt>
                      <dd>
                        <span className="badge">{me.provider}</span>
                      </dd>
                    </div>
                  )}
                </dl>
              ) : (
                <Loading />
              )}
            </div>
          )}

          {tab === 'service' && (
            <div className="tab-body">
              <p className="section-label">Runtime</p>
              {!cfg ? (
                <Loading />
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
                    <dd>
                      <code className="mono-inline">{cfg.listen}</code>
                    </dd>
                  </div>
                  <div>
                    <dt>Public URL</dt>
                    <dd>
                      <code className="mono-inline">{cfg.public_url || '—'}</code>
                      <p className="hint muted" style={{ marginTop: 6 }}>
                        From <code>MGMT_PUBLIC_URL</code> (required when listen is{' '}
                        <code>0.0.0.0</code>). Applied on cluster create as{' '}
                        <code>machine.dashboard.mgmt_url</code> (serial console).
                        Example: <code>https://mgmt.example.com</code>.
                      </p>
                    </dd>
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
                  <div>
                    <dt>Metrics TLS</dt>
                    <dd>
                      <BoolBadge on={cfg.metrics_tls_configured} />
                    </dd>
                  </div>
                </dl>
              )}
            </div>
          )}

          {tab === 'paths' && (
            <div className="tab-body">
              <p className="section-label">Filesystem</p>
              {!cfg ? (
                <Loading />
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
          )}

          {tab === 'auth' && (
            <div className="tab-body">
              <p className="section-label">Identity providers</p>
              {!cfg ? (
                <Loading />
              ) : (
                <>
                  <dl className="kv settings-kv">
                    <div>
                      <dt>Mode</dt>
                      <dd>
                        <span className="badge">{cfg.auth.mode}</span>
                      </dd>
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
                          <dd className="mono-inline">
                            {cfg.auth.auth0_domain || '—'}
                          </dd>
                        </div>
                        <div>
                          <dt>Client ID</dt>
                          <dd className="mono-inline">
                            {cfg.auth.auth0_client_id || '—'}
                          </dd>
                        </div>
                        <div>
                          <dt>Audience</dt>
                          <dd className="mono-inline">
                            {cfg.auth.auth0_audience || '—'}
                          </dd>
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
          )}

          {tab === 'email' && (
            <div className="tab-body">
              <p className="section-label">SMTP relay</p>
              {!cfg ? (
                <Loading />
              ) : (
                <>
                  <dl className="kv settings-kv">
                    <div>
                      <dt>SMTP</dt>
                      <dd>
                        <BoolBadge
                          on={cfg.smtp?.configured}
                          onLabel="configured"
                          offLabel="not configured"
                        />
                      </dd>
                    </div>
                    {cfg.smtp?.configured && (
                      <>
                        <div>
                          <dt>Host</dt>
                          <dd className="mono-inline">{cfg.smtp.host || '—'}</dd>
                        </div>
                        <div>
                          <dt>Port</dt>
                          <dd className="mono-inline">{cfg.smtp.port ?? '—'}</dd>
                        </div>
                        <div>
                          <dt>From</dt>
                          <dd className="mono-inline">{cfg.smtp.from || '—'}</dd>
                        </div>
                        <div>
                          <dt>TLS</dt>
                          <dd>
                            <span className="badge">{cfg.smtp.tls || '—'}</span>
                          </dd>
                        </div>
                      </>
                    )}
                    <div>
                      <dt>Admin notice emails</dt>
                      <dd>
                        <BoolBadge
                          on={cfg.smtp?.admin_emails_configured}
                          onLabel={`${cfg.smtp?.admin_email_count || 0} recipient(s)`}
                          offLabel="not set"
                        />
                      </dd>
                    </div>
                  </dl>
                  <p className="muted" style={{ marginTop: '1rem', marginBottom: 0 }}>
                    Configure via env: <code className="mono-inline">MGMT_SMTP_HOST</code>,{' '}
                    <code className="mono-inline">MGMT_SMTP_FROM</code>,{' '}
                    <code className="mono-inline">MGMT_SMTP_*</code>,{' '}
                    <code className="mono-inline">MGMT_ADMIN_EMAILS</code>.
                    Used for local password reset and Auth0 first-login notices.
                  </p>
                </>
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
