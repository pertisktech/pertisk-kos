import { useEffect, useState } from 'react'
import { api } from '../api'
import { Icon } from './Icons'
import WizardModal from './WizardModal'

const emptyProxmox = {
  kind: 'proxmox',
  name: 'lab-pve',
  url: 'https://10.1.1.197:8006',
  token_id: '',
  token_secret: '',
  node: 'pve',
  storage: 'local-lvm',
  bridge: 'vmbr0',
  insecure: true,
  arch: 'auto',
}

const emptyVsphere = {
  kind: 'vsphere',
  name: 'lab-esxi',
  url: 'https://10.1.1.20',
  token_id: 'root',
  token_secret: '',
  node: 'localhost.lan',
  storage: 'datastore1',
  bridge: 'VM Network',
  insecure: true,
  arch: 'auto',
}

const emptyNutanix = {
  kind: 'nutanix',
  name: 'lab-ahv',
  url: 'https://10.1.1.50:9440',
  token_id: 'admin',
  token_secret: '',
  node: 'NTNX-Cluster',
  storage: 'SelfServiceContainer',
  bridge: 'vlan.0',
  insecure: true,
  arch: 'auto',
}

function emptyForKind(kind) {
  if (kind === 'vsphere') return { ...emptyVsphere }
  if (kind === 'nutanix') return { ...emptyNutanix }
  return { ...emptyProxmox }
}

function isUserPass(kind) {
  return kind === 'vsphere' || kind === 'nutanix'
}

function labelsFor(kind) {
  if (kind === 'vsphere') {
    return {
      product: 'ESXi',
      hosts: 'hosts',
      host: 'host',
      storage: 'datastore',
      network: 'Network',
      nodeField: 'Host',
      credUser: 'Username',
      credPass: 'Password',
      urlPh: 'https://10.1.1.20',
      userPh: 'root',
      netPh: 'VM Network',
    }
  }
  if (kind === 'nutanix') {
    return {
      product: 'Nutanix',
      hosts: 'hosts',
      host: 'cluster/host',
      storage: 'storage container',
      network: 'Network',
      nodeField: 'Cluster / host',
      credUser: 'Username',
      credPass: 'Password',
      urlPh: 'https://10.1.1.50:9440',
      userPh: 'admin',
      netPh: 'vlan.0',
    }
  }
  return {
    product: 'Proxmox',
    hosts: 'nodes',
    host: 'node',
    storage: 'storage',
    network: 'Bridge',
    nodeField: 'Node',
    credUser: 'Token ID',
    credPass: 'Token secret',
    urlPh: 'https://10.1.1.197:8006',
    userPh: 'root@pam!pertisk',
    netPh: 'vmbr0',
  }
}

function formatProbe(r, kind) {
  const L = labelsFor(kind)
  const nodes = (r.nodes || []).map((n) => n.node).join(', ') || '(none)'
  const parts = [
    `${L.product} ${r.version || '?'} @ ${r.url}`,
    `${L.hosts}: ${nodes}`,
    r.node_ok
      ? `${L.host} OK (${r.node_message || 'ok'})`
      : `${L.host} FAIL: ${r.node_message || 'unknown'}`,
  ]
  if (r.arch) {
    parts.push(`guest arch → ${r.arch}`)
  }
  if (r.storage) {
    parts.push(
      r.storage.ok
        ? `${L.storage} OK: ${r.storage.storage} (${r.storage.type_ || r.storage.type || '?'})`
        : `${L.storage} FAIL: ${r.storage.message}`,
    )
  }
  return parts.join(' — ')
}

const STEPS = [
  { id: 'connection', label: 'Connection' },
  { id: 'placement', label: 'Placement' },
  { id: 'review', label: 'Test & save' },
]

export default function ProviderWizard({ open, mode = 'create', provider = null, onClose, onSaved }) {
  const editingId = mode === 'edit' && provider ? provider.id : 'new'
  const [step, setStep] = useState(0)
  const [form, setForm] = useState({ ...emptyProxmox })
  const [storageOptions, setStorageOptions] = useState([])
  const [error, setError] = useState('')
  const [msg, setMsg] = useState('')
  const [saving, setSaving] = useState(false)
  const [testing, setTesting] = useState(false)

  useEffect(() => {
    if (!open) return
    setStep(0)
    setError('')
    setMsg('')
    setSaving(false)
    setTesting(false)
    if (mode === 'edit' && provider) {
      setStorageOptions(provider.storage ? [provider.storage] : [])
      const kind = provider.kind || 'proxmox'
      const L = labelsFor(kind)
      setForm({
        kind,
        name: provider.name,
        url: provider.url,
        token_id: provider.token_id,
        token_secret: '',
        node: provider.node,
        storage: provider.storage,
        bridge: provider.bridge || L.netPh,
        insecure: !!provider.insecure,
        arch: provider.arch === 'arm64' || provider.arch === 'amd64' ? provider.arch : 'auto',
      })
    } else {
      setStorageOptions([])
      setForm({ ...emptyProxmox })
    }
  }, [open, mode, provider])

  function set(k, v) {
    setForm((f) => ({ ...f, [k]: v }))
  }

  function setKind(kind) {
    setStorageOptions([])
    setForm((f) => {
      const base = emptyForKind(kind)
      return {
        ...base,
        name: f.name || base.name,
        token_secret: f.token_secret,
      }
    })
  }

  function applyProbeResult(r) {
    const available = r.storage?.available || []
    setForm((f) => {
      let next = { ...f }
      if (available.length && f.storage && !available.includes(f.storage)) {
        next = { ...next, storage: available[0] }
      }
      if (r.arch && (f.arch === 'auto' || !f.arch)) {
        next = { ...next, arch: r.arch }
      }
      return next
    })
    if (available.length) {
      setStorageOptions(available)
    } else if (r.storage?.storage) {
      setStorageOptions([r.storage.storage])
    }
    const text = formatProbe(r, form.kind)
    if (r.ok) {
      setMsg(`OK — ${text}`)
      setError('')
    } else {
      setMsg('')
      setError(text)
    }
  }

  function validateStep(i) {
    const L = labelsFor(form.kind)
    if (i === 0) {
      if (!form.name.trim()) return 'Name is required'
      if (!form.url.trim()) return 'URL is required'
      if (!form.token_id.trim()) {
        return `${L.credUser} is required`
      }
      if (editingId === 'new' && !form.token_secret) {
        return `${L.credPass} is required`
      }
    }
    if (i === 1) {
      if (!form.node.trim()) {
        return `${L.nodeField} is required`
      }
      if (!form.storage.trim()) {
        return `${L.storage} is required`
      }
    }
    return ''
  }

  function next() {
    const e = validateStep(step)
    if (e) {
      setError(e)
      return
    }
    setError('')
    setMsg('')
    setStep((s) => Math.min(s + 1, STEPS.length - 1))
  }

  function back() {
    setError('')
    setMsg('')
    setStep((s) => Math.max(s - 1, 0))
  }

  async function testDraft() {
    const L = labelsFor(form.kind)
    setError('')
    setMsg('Testing…')
    setTesting(true)
    try {
      if (editingId === 'new' || form.token_secret) {
        if (!form.token_secret) {
          throw new Error(`${L.credPass} is required to test`)
        }
        const r = await api('/providers/probe', {
          method: 'POST',
          body: {
            kind: form.kind,
            name: form.name,
            url: form.url,
            token_id: form.token_id,
            token_secret: form.token_secret,
            node: form.node,
            storage: form.storage,
            bridge: form.bridge,
            insecure: form.insecure,
          },
        })
        applyProbeResult(r)
      } else {
        const r = await api(`/providers/${editingId}/test`, {
          method: 'POST',
          body: {
            node: form.node,
            storage: form.storage,
            bridge: form.bridge,
          },
        })
        applyProbeResult(r)
      }
    } catch (err) {
      setMsg('')
      setError(err.message)
    } finally {
      setTesting(false)
    }
  }

  async function save() {
    const e0 = validateStep(0) || validateStep(1)
    if (e0) {
      setError(e0)
      setStep(
        /Host|Node|Cluster|Storage|Datastore|container/i.test(e0) ? 1 : 0,
      )
      return
    }
    const L = labelsFor(form.kind)
    setError('')
    setMsg('')
    setSaving(true)
    try {
      if (editingId === 'new') {
        if (!form.token_secret) {
          throw new Error(`${L.credPass} is required`)
        }
        await api('/providers', { method: 'POST', body: form })
      } else {
        const body = {
          name: form.name,
          url: form.url,
          token_id: form.token_id,
          node: form.node,
          storage: form.storage,
          bridge: form.bridge,
          insecure: form.insecure,
          arch: form.arch,
        }
        if (form.token_secret) body.token_secret = form.token_secret
        await api(`/providers/${editingId}`, { method: 'PUT', body })
      }
      onSaved?.()
      onClose?.()
    } catch (err) {
      setError(err.message)
    } finally {
      setSaving(false)
    }
  }

  const L = labelsFor(form.kind)
  const busy = saving || testing
  const storageList = storageOptions.length
    ? storageOptions
    : form.storage
      ? [form.storage]
      : []
  const kindTitle =
    form.kind === 'vsphere' ? 'vSphere' : form.kind === 'nutanix' ? 'Nutanix' : 'Proxmox'
  const title = mode === 'edit' ? `Edit ${kindTitle} provider` : `Add ${kindTitle} provider`

  return (
    <WizardModal
      open={open}
      title={title}
      icon={mode === 'edit' ? 'edit' : 'plus'}
      onClose={onClose}
      steps={STEPS}
      stepIndex={step}
      onStepChange={(i) => {
        setError('')
        setMsg('')
        setStep(i)
      }}
      footer={
        <>
          <button type="button" className="secondary" onClick={step === 0 ? onClose : back} disabled={busy}>
            {step === 0 ? 'Cancel' : 'Back'}
          </button>
          <div className="wizard-footer-right">
            {step < STEPS.length - 1 ? (
              <button type="button" onClick={next} disabled={busy}>
                Next
              </button>
            ) : (
              <>
                <button type="button" className="secondary btn-icon" onClick={testDraft} disabled={busy}>
                  <Icon name="play" size={16} /> {testing ? 'Testing…' : 'Test'}
                </button>
                <button type="button" className="btn-icon" onClick={save} disabled={busy}>
                  <Icon name="check" size={16} /> {saving ? 'Saving…' : 'Save'}
                </button>
              </>
            )}
          </div>
        </>
      }
    >
      {error && <div className="error">{error}</div>}
      {msg && <p className="muted">{msg}</p>}

      {step === 0 && (
        <>
          <p className="wizard-section-title">Connection</p>
          <p className="muted" style={{ marginTop: 0, fontSize: '0.85rem' }}>
            {isUserPass(form.kind) ? (
              <>Lab {kindTitle} often uses a self-signed cert — keep Insecure TLS on. Leave password blank when editing to keep the existing secret.</>
            ) : (
              <>Lab Proxmox often uses a self-signed cert — keep Insecure TLS on. Leave token secret blank when editing to keep the existing secret.</>
            )}
          </p>
          <div className="form-grid">
            {editingId === 'new' && (
              <div className="field">
                <label>Kind</label>
                <select value={form.kind} onChange={(e) => setKind(e.target.value)}>
                  <option value="proxmox">Proxmox</option>
                  <option value="vsphere">vSphere (ESXi)</option>
                  <option value="nutanix">Nutanix (AHV)</option>
                </select>
              </div>
            )}
            <div className="field">
              <label>Name</label>
              <input value={form.name} onChange={(e) => set('name', e.target.value)} autoFocus />
            </div>
            <div className="field">
              <label>URL</label>
              <input
                value={form.url}
                onChange={(e) => set('url', e.target.value)}
                placeholder={L.urlPh}
              />
            </div>
            <div className="field">
              <label>{L.credUser}</label>
              <input
                value={form.token_id}
                onChange={(e) => set('token_id', e.target.value)}
                placeholder={L.userPh}
              />
            </div>
            <div className="field">
              <label>
                {L.credPass}
                {editingId !== 'new' ? ' (leave blank to keep)' : ''}
              </label>
              <input
                type="password"
                value={form.token_secret}
                onChange={(e) => set('token_secret', e.target.value)}
                placeholder={editingId !== 'new' ? '••••••••' : ''}
                autoComplete="new-password"
              />
            </div>
            <div className="field">
              <label>Insecure TLS</label>
              <select value={form.insecure ? '1' : '0'} onChange={(e) => set('insecure', e.target.value === '1')}>
                <option value="1">Yes (lab / self-signed)</option>
                <option value="0">No</option>
              </select>
            </div>
          </div>
        </>
      )}

      {step === 1 && (
        <>
          <p className="wizard-section-title">Placement</p>
          <div className="form-grid">
            <div className="field">
              <label>{L.nodeField}</label>
              <input value={form.node} onChange={(e) => set('node', e.target.value)} />
            </div>
            <div className="field">
              <label>{form.kind === 'vsphere' ? 'Datastore' : form.kind === 'nutanix' ? 'Storage container' : 'Storage'}</label>
              {storageList.length > 1 ? (
                <select value={form.storage} onChange={(e) => set('storage', e.target.value)}>
                  {storageList.map((s) => (
                    <option key={s} value={s}>{s}</option>
                  ))}
                </select>
              ) : (
                <input
                  value={form.storage}
                  onChange={(e) => set('storage', e.target.value)}
                  list="provider-wizard-storage-options"
                />
              )}
              <datalist id="provider-wizard-storage-options">
                {storageList.map((s) => (
                  <option key={s} value={s} />
                ))}
              </datalist>
              <p className="hint muted">
                {form.kind === 'vsphere'
                  ? 'Run Test on the last step to discover datastores.'
                  : form.kind === 'nutanix'
                    ? 'Run Test on the last step to discover storage containers.'
                    : 'Run Test on the last step to discover storages that support images.'}
              </p>
            </div>
            <div className="field">
              <label>{L.network}</label>
              <input
                value={form.bridge}
                onChange={(e) => set('bridge', e.target.value)}
                placeholder={L.netPh}
              />
            </div>
            <div className="field">
              <label>Guest arch</label>
              <select value={form.arch} onChange={(e) => set('arch', e.target.value)}>
                <option value="auto">Auto (detect from host)</option>
                <option value="amd64">amd64 (x86_64)</option>
                <option value="arm64">arm64 (aarch64)</option>
              </select>
              <p className="hint muted">
                Auto reads the host via Test/Save. Override only for cross-arch guests.
              </p>
            </div>
          </div>
        </>
      )}

      {step === 2 && (
        <>
          <p className="wizard-section-title">Review</p>
          <div className="form-grid">
            <div className="field">
              <label>Name</label>
              <div>{form.name || '—'}</div>
            </div>
            <div className="field">
              <label>Kind</label>
              <div>{form.kind}</div>
            </div>
            <div className="field">
              <label>URL</label>
              <div className="mono">{form.url || '—'}</div>
            </div>
            <div className="field">
              <label>{L.nodeField}</label>
              <div>{form.node || '—'}</div>
            </div>
            <div className="field">
              <label>{form.kind === 'vsphere' ? 'Datastore' : form.kind === 'nutanix' ? 'Storage container' : 'Storage'}</label>
              <div>{form.storage || '—'}</div>
            </div>
            <div className="field">
              <label>{L.network}</label>
              <div>{form.bridge || '—'}</div>
            </div>
            <div className="field">
              <label>Guest arch</label>
              <div>{form.arch || 'auto'}</div>
            </div>
            <div className="field">
              <label>Insecure TLS</label>
              <div>{form.insecure ? 'Yes' : 'No'}</div>
            </div>
          </div>
        </>
      )}
    </WizardModal>
  )
}
