import { useCallback, useEffect, useState } from 'react'
import { Icon } from '../../components/Icons'
import { useConfirm } from '../../components/Confirm'
import { checkAddon, installAddon, listAddons } from './api'

function statusLabel(status) {
  switch (status) {
    case 'installed':
      return 'Installed'
    case 'installing':
      return 'Installing'
    case 'partial':
      return 'Partial'
    case 'missing':
      return 'Missing'
    case 'error':
      return 'Error'
    default:
      return 'Not installed'
  }
}

function badgeClass(status) {
  if (status === 'installed') return 'badge ready'
  if (status === 'installing' || status === 'partial') return 'badge running'
  if (status === 'error' || status === 'missing') return 'badge error'
  return 'badge'
}

function emptyForm(addon) {
  const next = {}
  for (const f of addon.fields || []) {
    if (f.kind === 'select' && f.options?.length) {
      next[f.name] = addon.config?.[f.name] || f.options[0]
    } else {
      next[f.name] = addon.config?.[f.name] || ''
    }
  }
  return next
}

function fieldVisible(addon, f) {
  if (addon.id !== 'cilium-lb') return true
  if (f.name === 'ipv6' && addon.network_mode === 'ipv4') return false
  if (f.name === 'ipv4' && addon.network_mode === 'ipv6') return false
  return true
}

function AddonCard({ clusterId, addon, onInstalled }) {
  const confirm = useConfirm()
  const [form, setForm] = useState(() => emptyForm(addon))
  const [busy, setBusy] = useState('')
  const [check, setCheck] = useState(null)
  const [error, setError] = useState('')
  const savedKey = `${addon.id}:${addon.updated_at || ''}:${addon.token_set ? 1 : 0}:${JSON.stringify(addon.config || {})}`

  useEffect(() => {
    setForm((prev) => {
      const next = emptyForm(addon)
      for (const f of addon.fields || []) {
        if (f.kind === 'password' && prev[f.name]) next[f.name] = prev[f.name]
      }
      return next
    })
    // Re-hydrate when the saved cluster config changes, not on every parent poll.
    // eslint-disable-next-line react-hooks/exhaustive-deps -- savedKey captures config
  }, [savedKey])

  function setField(name, value) {
    setForm((prev) => ({ ...prev, [name]: value }))
  }

  async function onCheck() {
    setBusy('check')
    setError('')
    try {
      const res = await checkAddon(clusterId, addon.id, form)
      setCheck(res)
    } catch (e) {
      setError(e.message || 'check failed')
    } finally {
      setBusy('')
    }
  }

  async function onInstall() {
    const ok = await confirm({
      title: addon.status === 'installed' ? 'Update add-on' : 'Install add-on',
      message: `Apply ${addon.name} to this cluster with the config below?`,
      confirmLabel: addon.status === 'installed' ? 'Update' : 'Install',
      tone: 'primary',
    })
    if (!ok) return
    setBusy('install')
    setError('')
    try {
      const res = await installAddon(clusterId, addon.id, form)
      onInstalled?.(res)
    } catch (e) {
      setError(e.message || 'install failed')
    } finally {
      setBusy('')
    }
  }

  const result = check || addon
  const live = result.live || {}
  const warnings = result.warnings || []
  const errors = result.errors || []

  return (
    <article className="addon-card">
      <div className="addon-card-head">
        <div>
          <h3>{addon.name}</h3>
          <p className="muted">{addon.summary}</p>
        </div>
        <span className={badgeClass(addon.status)}>{statusLabel(addon.status)}</span>
      </div>

      {addon.error && <div className="error">{addon.error}</div>}
      {error && <div className="error">{error}</div>}

      <div className="form-grid">
        {(addon.fields || []).filter((f) => fieldVisible(addon, f)).map((f) => (
          <label key={f.name} className="field">
            {f.label}
            {f.kind === 'select' ? (
              <select
                value={form[f.name] || f.options?.[0] || ''}
                onChange={(e) => setField(f.name, e.target.value)}
              >
                {(f.options || []).map((opt) => (
                  <option key={opt} value={opt}>
                    {opt}
                  </option>
                ))}
              </select>
            ) : (
              <input
                type={f.kind === 'password' ? 'password' : 'text'}
                value={form[f.name] || ''}
                placeholder={
                  f.kind === 'password' && (
                    f.name === 'registry_password' ? addon.registry_set : addon.token_set
                  )
                    ? 'unchanged'
                    : f.placeholder
                }
                autoComplete="off"
                onChange={(e) => setField(f.name, e.target.value)}
              />
            )}
            {f.help && <span className="muted field-help">{f.help}</span>}
          </label>
        ))}
      </div>

      <div className="addon-actions">
        <button
          type="button"
          className="secondary btn-icon"
          onClick={onCheck}
          disabled={!!busy}
        >
          <Icon name="check" size={14} />
          {busy === 'check' ? 'Checking…' : 'Check config'}
        </button>
        <button
          type="button"
          className="btn-icon"
          onClick={onInstall}
          disabled={!!busy || addon.status === 'installing'}
        >
          <Icon name="play" size={14} />
          {busy === 'install'
            ? 'Starting…'
            : addon.status === 'installed'
              ? 'Update'
              : 'Install'}
        </button>
      </div>

      {(errors.length > 0 || warnings.length > 0 || live.available) && (
        <div className="addon-check">
          {errors.map((msg) => (
            <div key={msg} className="error">{msg}</div>
          ))}
          {warnings.map((msg) => (
            <div key={msg} className="muted">{msg}</div>
          ))}
          {addon.id === 'nfs' && live.available && (
            <dl className="kv addon-live">
              <div><dt>Provisioner</dt><dd>{live.provisioner_ready ? 'ready' : live.installed ? 'not ready' : 'absent'}</dd></div>
              <div><dt>StorageClass</dt><dd>{live.storage_class ? 'nfs-client' : '—'}</dd></div>
              <div><dt>Live server</dt><dd className="mono-inline">{live.server || '—'}</dd></div>
              <div><dt>Live path</dt><dd className="mono-inline">{live.path || '—'}</dd></div>
            </dl>
          )}
          {addon.id === 'cert-manager' && live.available && (
            <dl className="kv addon-live">
              <div><dt>Controller</dt><dd>{live.controller_ready ? 'ready' : live.installed ? 'not ready' : 'absent'}</dd></div>
              <div><dt>Webhook</dt><dd>{live.webhook_ready ? 'ready' : '—'}</dd></div>
              <div><dt>ClusterIssuer</dt><dd>{live.issuer_ready ? 'ready' : live.issuer ? 'not ready' : 'absent'}</dd></div>
              <div><dt>Token secret</dt><dd>{live.token_secret ? 'present' : '—'}</dd></div>
              <div><dt>Version</dt><dd className="mono-inline">{live.version || addon.cert_manager_version || '—'}</dd></div>
            </dl>
          )}
          {addon.id === 'cilium-lb' && live.available && (
            <dl className="kv addon-live">
              <div><dt>IP pool</dt><dd>{live.pool ? 'present' : 'absent'}</dd></div>
              <div><dt>L2 policy</dt><dd>{live.l2 ? 'present' : 'absent'}</dd></div>
              <div><dt>Live IPv4</dt><dd className="mono-inline">{live.ipv4 || '—'}</dd></div>
              <div><dt>Live IPv6</dt><dd className="mono-inline">{live.ipv6 || '—'}</dd></div>
            </dl>
          )}
          {addon.id === 'ingress' && live.available && (
            <dl className="kv addon-live">
              <div><dt>Controller</dt><dd>{live.controller_ready ? 'ready' : live.installed ? 'not ready' : 'absent'}</dd></div>
              <div><dt>IngressClass</dt><dd>{live.ingress_class ? 'pertisk-proxy' : '—'}</dd></div>
              <div><dt>Image</dt><dd className="mono-inline">{live.image || addon.ingress_image || '—'}</dd></div>
              <div><dt>Pull secret</dt><dd>{live.pull_secret ? 'present' : 'absent'}</dd></div>
              <div><dt>LB IPv4</dt><dd className="mono-inline">{live.lb_ipv4 || '—'}</dd></div>
              <div><dt>LB IPv6</dt><dd className="mono-inline">{live.lb_ipv6 || '—'}</dd></div>
            </dl>
          )}
        </div>
      )}
    </article>
  )
}

export default function AddonsTab({ clusterId, ready, onInstalled }) {
  const [addons, setAddons] = useState([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')

  const load = useCallback(async () => {
    if (!clusterId) return
    setLoading(true)
    setError('')
    try {
      const res = await listAddons(clusterId)
      setAddons(res.data || [])
    } catch (e) {
      setError(e.message || 'failed to load add-ons')
      setAddons([])
    } finally {
      setLoading(false)
    }
  }, [clusterId])

  useEffect(() => {
    load()
  }, [load])

  useEffect(() => {
    if (!addons.some((a) => a.status === 'installing')) return undefined
    const t = setInterval(load, 4000)
    return () => clearInterval(t)
  }, [addons, load])

  if (!ready) {
    return (
      <div className="tab-body">
        <p className="muted">
          Add-ons can be installed when the cluster status is <span className="badge ready">ready</span>
          {' '}and a kubeconfig has been stored.
        </p>
      </div>
    )
  }

  return (
    <div className="tab-body tab-body-fill addons-tab">
      <div className="section-head">
        <div>
          <h3 className="section-label">Add-ons</h3>
          <p className="muted">
            Check config, then install into this cluster via kubectl or Helm on the management host.
          </p>
        </div>
        <button type="button" className="secondary btn-icon" onClick={load} disabled={loading}>
          <Icon name="refresh" size={14} /> Refresh
        </button>
      </div>

      {error && <div className="error">{error}</div>}

      <div className="addon-grid">
        {addons.map((addon) => (
          <AddonCard
            key={addon.id}
            clusterId={clusterId}
            addon={addon}
            onInstalled={onInstalled}
          />
        ))}
      </div>
    </div>
  )
}
