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

function fieldVisible(addon, f, form) {
  if (addon.id === 'cilium-lb') {
    if (f.name === 'ipv6' && addon.network_mode === 'ipv4') return false
    if (f.name === 'ipv4' && addon.network_mode === 'ipv6') return false
  }
  if (addon.id === 'ingress' && f.name === 'tls_secret') {
    return !!(form.admin_host || '').trim()
  }
  return true
}

function selectOptions(f, form) {
  const opts = [...(f.options || [])]
  const cur = form[f.name]
  if (cur && !opts.includes(cur)) opts.push(cur)
  return opts
}

function optionLabel(f, opt) {
  if (f.name === 'tls_secret' && opt === 'none') return 'none (HTTP only)'
  return opt
}

const ADDON_SECTIONS = [
  {
    id: 'autoscaling',
    title: 'Autoscaling',
    blurb: 'Scale workers through pertisk-mgmt when pods are pending or utilization is high.',
  },
  {
    id: 'certificates',
    title: 'Certificates',
    blurb: 'Issue a wildcard TLS certificate and copy it into every namespace.',
  },
  {
    id: 'ingress',
    title: 'Ingress',
    blurb: 'Expose services through pertisk-proxy. Pick a TLS secret when you set an admin domain.',
  },
  {
    id: 'cluster',
    title: 'Storage & network',
    blurb: 'NFS volumes and Cilium LoadBalancer IPs.',
  },
]

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
    setForm((prev) => {
      const next = { ...prev, [name]: value }
      if (name === 'admin_host' && !value.trim()) next.tls_secret = 'none'
      return next
    })
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
        {(addon.fields || []).filter((f) => fieldVisible(addon, f, form)).map((f) => (
          <label key={f.name} className="field">
            {f.label}
            {f.kind === 'select' ? (
              <select
                value={form[f.name] || f.options?.[0] || ''}
                onChange={(e) => setField(f.name, e.target.value)}
              >
                {selectOptions(f, form).map((opt) => (
                  <option key={opt} value={opt}>
                    {optionLabel(f, opt)}
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
              <div><dt>Reflector</dt><dd>{live.reflector_ready ? 'ready' : live.reflector ? 'not ready' : 'absent'}</dd></div>
              <div><dt>Wildcard cert</dt><dd>{live.certificate_ready ? 'ready' : live.certificate ? 'not ready' : (addon.config?.domain ? 'absent' : '—')}</dd></div>
              <div><dt>TLS secret</dt><dd className="mono-inline">{live.tls_secret || '—'}</dd></div>
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
              <div><dt>Admin host</dt><dd className="mono-inline">{live.admin_host || '—'}</dd></div>
              <div><dt>Admin TLS</dt><dd className="mono-inline">{live.admin_host ? (live.admin_tls_secret || (live.admin_tls ? 'yes' : 'none')) : '—'}</dd></div>
            </dl>
          )}
        </div>
      )}
    </article>
  )
}

function sectionItems(addons, sectionId) {
  return addons.filter((a) => (a.section || 'cluster') === sectionId)
}

function sectionStatus(items) {
  if (items.some((a) => a.status === 'installing')) return 'installing'
  if (items.some((a) => a.status === 'error' || a.status === 'missing')) return 'error'
  if (items.length > 0 && items.every((a) => a.status === 'installed')) return 'installed'
  if (items.some((a) => a.status === 'installed' || a.status === 'partial')) return 'partial'
  return ''
}

export default function AddonsTab({ clusterId, ready, onInstalled }) {
  const [addons, setAddons] = useState([])
  const [group, setGroup] = useState('certificates')
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

  const visibleSections = ADDON_SECTIONS.filter((s) => sectionItems(addons, s.id).length > 0)
  const visibleIds = visibleSections.map((s) => s.id).join(',')

  useEffect(() => {
    if (!visibleIds) return
    const ids = visibleIds.split(',')
    if (!ids.includes(group)) setGroup(ids[0])
  }, [group, visibleIds])

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

  const active = visibleSections.find((s) => s.id === group) || visibleSections[0]
  const items = active ? sectionItems(addons, active.id) : []

  return (
    <div className="tab-body tab-body-fill addons-tab">
      <div className="section-head">
        <div>
          <h3 className="section-label">Add-ons</h3>
          <p className="muted">
            Check config, then install into this cluster via kubectl or Helm on the management host.
            Config is saved by cluster name (including tokens) and reused when you recreate the cluster.
            Add-on jobs run in parallel with other clusters (they do not wait in the global create queue).
          </p>
        </div>
        <button type="button" className="secondary btn-icon" onClick={load} disabled={loading}>
          <Icon name="refresh" size={14} /> Refresh
        </button>
      </div>

      {error && <div className="error">{error}</div>}

      {visibleSections.length > 0 && (
        <div className="addon-groups" role="tablist" aria-label="Add-on groups">
          {visibleSections.map((section) => {
            const count = sectionItems(addons, section.id).length
            const status = sectionStatus(sectionItems(addons, section.id))
            return (
              <button
                key={section.id}
                type="button"
                role="tab"
                aria-selected={active?.id === section.id}
                className={active?.id === section.id ? 'tab-btn active' : 'tab-btn'}
                onClick={() => setGroup(section.id)}
              >
                <span>{section.title}</span>
                <span className="tab-count">{count}</span>
                {status ? (
                  <span className={`${badgeClass(status)} addon-group-badge`}>
                    {statusLabel(status)}
                  </span>
                ) : null}
              </button>
            )
          })}
        </div>
      )}

      {active && (
        <section className="addon-section" role="tabpanel">
          <p className="muted addon-section-blurb">{active.blurb}</p>
          <div className="addon-grid">
            {items.map((addon) => (
              <AddonCard
                key={addon.id}
                clusterId={clusterId}
                addon={addon}
                onInstalled={onInstalled}
              />
            ))}
          </div>
        </section>
      )}
    </div>
  )
}
