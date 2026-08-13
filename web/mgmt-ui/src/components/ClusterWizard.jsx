import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { api } from '../api'
import { Icon } from './Icons'
import K8sVersionSelect from './K8sVersionSelect'
import WizardModal from './WizardModal'

function VerifyRow({ state, label, message }) {
  let icon = null
  if (state === 'loading') {
    icon = <span className="spinner" aria-hidden />
  } else if (state === 'ok') {
    icon = <Icon name="check" size={14} />
  } else if (state === 'error') {
    icon = <Icon name="alert" size={14} />
  } else if (state === 'skipped') {
    icon = <Icon name="check" size={14} />
  }

  return (
    <li className={`verify-row ${state}`} aria-live={state === 'loading' ? 'polite' : undefined}>
      <span className="verify-row-icon">{icon}</span>
      <span className="verify-row-label">{label}</span>
      <p className="verify-row-msg">{message}</p>
    </li>
  )
}

const STEPS = [
  { id: 'general', label: 'General' },
  { id: 'network', label: 'Network' },
  { id: 'size', label: 'Size' },
  { id: 'verify', label: 'Verify' },
]

const defaultForm = {
  name: 'lab-ha',
  provider_id: '',
  controlplanes: 3,
  workers: 2,
  network_mode: 'ipv4',
  arch: 'amd64',
  vip: '10.1.1.250',
  vip6: 'fd00:1::250',
  cni: 'cilium',
  k8s_version: '',
  max_pods: 250,
  pod_subnet: '10.244.0.0/16',
  service_subnet: '10.96.0.0/12',
  pod_subnet_ipv6: '2001:db8:10:0::/56',
  service_subnet_ipv6: '2001:db8:96:1::/112',
  cp_memory: 4096,
  cp_cores: 2,
  cp_disk_gb: 50,
  worker_memory: 8192,
  worker_cores: 4,
  worker_disk_gb: 75,
  cp_vmid: 210,
}

export default function ClusterWizard({ open, onClose, onCreated }) {
  const nav = useNavigate()
  const [step, setStep] = useState(0)
  const [providers, setProviders] = useState([])
  const [error, setError] = useState('')
  const [saving, setSaving] = useState(false)
  const [vmidCheck, setVmidCheck] = useState(null)
  const [vmidChecking, setVmidChecking] = useState(false)
  const [vipCheck, setVipCheck] = useState(null)
  const [vipChecking, setVipChecking] = useState(false)
  const [form, setForm] = useState({ ...defaultForm })

  useEffect(() => {
    if (!open) return
    let cancelled = false
    setStep(0)
    setError('')
    setSaving(false)
    setVmidCheck(null)
    setVipCheck(null)
    setForm({ ...defaultForm })
    api('/providers')
      .then(async (p) => {
        if (cancelled) return
        setProviders(p)
        if (!p[0]) return
        const first = p[0]
        const arch = first.arch === 'arm64' ? 'arm64' : 'amd64'
        // Apply first provider immediately so a slow suggest-vmid cannot later
        // stomp a provider the user already chose.
        setForm((f) => {
          if (f.provider_id && f.provider_id !== first.id) return f
          return { ...f, provider_id: first.id, arch }
        })
        try {
          const sug = await api('/clusters/suggest-vmid', {
            method: 'POST',
            body: {
              provider_id: first.id,
              cp_vmid: defaultForm.cp_vmid,
              controlplanes: defaultForm.controlplanes,
              workers: defaultForm.workers,
            },
          })
          if (cancelled || !sug?.cp_vmid) return
          setForm((f) => (f.provider_id === first.id ? { ...f, cp_vmid: sug.cp_vmid } : f))
        } catch {
          /* keep default 210; check-vmids will still validate */
        }
      })
      .catch((e) => {
        if (!cancelled) setError(e.message)
      })
    return () => {
      cancelled = true
    }
  }, [open])

  useEffect(() => {
    if (!open) return
    const providerId = form.provider_id
    const cpVmid = Number(form.cp_vmid)
    const cps = Number(form.controlplanes)
    const workers = Number(form.workers)
    const count = cps + workers
    if (!providerId || !Number.isFinite(cpVmid) || cpVmid < 1 || count < 1) {
      setVmidCheck(null)
      setVmidChecking(false)
      return
    }
    let cancelled = false
    setVmidChecking(true)
    const t = setTimeout(() => {
      api('/clusters/check-vmids', {
        method: 'POST',
        body: {
          provider_id: providerId,
          cp_vmid: cpVmid,
          controlplanes: cps,
          workers,
        },
      })
        .then((r) => {
          if (!cancelled) setVmidCheck(r)
        })
        .catch((err) => {
          if (!cancelled) {
            setVmidCheck({
              ok: false,
              message: err.message,
              conflicts: [],
              free: [],
              range_start: cpVmid,
              range_end: cpVmid + count - 1,
            })
          }
        })
        .finally(() => {
          if (!cancelled) setVmidChecking(false)
        })
    }, 400)
    return () => {
      cancelled = true
      clearTimeout(t)
    }
  }, [open, form.provider_id, form.cp_vmid, form.controlplanes, form.workers])

  useEffect(() => {
    if (!open) return
    const cps = Number(form.controlplanes)
    const mode = form.network_mode
    if (cps <= 1) {
      setVipCheck(null)
      setVipChecking(false)
      return
    }
    const vip =
      mode === 'ipv4' || mode === 'dual-stack' ? String(form.vip || '').trim() : ''
    const vip6 =
      mode === 'ipv6' || mode === 'dual-stack' ? String(form.vip6 || '').trim() : ''
    if (!vip && !vip6) {
      setVipCheck(null)
      setVipChecking(false)
      return
    }
    let cancelled = false
    setVipChecking(true)
    const t = setTimeout(() => {
      api('/clusters/check-vip', {
        method: 'POST',
        body: {
          vip: vip || null,
          vip6: vip6 || null,
        },
      })
        .then((r) => {
          if (!cancelled) setVipCheck(r)
        })
        .catch((err) => {
          if (!cancelled) {
            setVipCheck({ ok: false, message: err.message, conflicts: [] })
          }
        })
        .finally(() => {
          if (!cancelled) setVipChecking(false)
        })
    }, 400)
    return () => {
      cancelled = true
      clearTimeout(t)
    }
  }, [open, form.controlplanes, form.network_mode, form.vip, form.vip6])

  function set(k, v) {
    setForm((f) => {
      const next = { ...f, [k]: v }
      if (k === 'controlplanes' && Number(v) <= 1) {
        next.vip = ''
        next.vip6 = ''
      }
      if (k === 'provider_id') {
        const p = providers.find((x) => x.id === v)
        if (p?.arch === 'arm64' || p?.arch === 'amd64') next.arch = p.arch
      }
      return next
    })
    if (k === 'provider_id' && v) {
      const selected = v
      api('/clusters/suggest-vmid', {
        method: 'POST',
        body: {
          provider_id: selected,
          cp_vmid: defaultForm.cp_vmid,
          controlplanes: Number(form.controlplanes) || 1,
          workers: Number(form.workers) || 1,
        },
      })
        .then((sug) => {
          if (!sug?.cp_vmid) return
          setForm((f) => (f.provider_id === selected ? { ...f, cp_vmid: sug.cp_vmid } : f))
        })
        .catch(() => {})
    }
  }

  const ha = Number(form.controlplanes) > 1
  const mode = form.network_mode
  const vmidBlocked = vmidCheck && !vmidCheck.ok
  const vipBlocked = ha && vipCheck && !vipCheck.ok
  const verifying = vmidChecking || (ha && vipChecking)
  const selectedProvider = providers.find((p) => p.id === form.provider_id)

  const providerVerify = !form.provider_id
    ? { state: 'idle', message: 'Select a provider' }
    : {
        state: 'ok',
        message: selectedProvider
          ? `${selectedProvider.name} (${form.arch})`
          : 'Provider selected',
      }

  const k8sVerify = !String(form.k8s_version || '').trim()
    ? { state: 'idle', message: 'Select a Kubernetes version' }
    : { state: 'ok', message: form.k8s_version }

  let vmidVerify = { state: 'idle', message: 'Waiting for provider / base VMID' }
  if (!form.provider_id) {
    vmidVerify = { state: 'idle', message: 'Waiting for provider' }
  } else if (vmidChecking) {
    vmidVerify = { state: 'loading', message: 'Checking VMIDs on provider…' }
  } else if (vmidCheck?.ok) {
    vmidVerify = {
      state: 'ok',
      message: `VMIDs ${vmidCheck.range_start}–${vmidCheck.range_end} free on ${vmidCheck.node}`,
    }
  } else if (vmidCheck && !vmidCheck.ok) {
    const conflicts =
      vmidCheck.conflicts?.length > 0
        ? ` In use: ${vmidCheck.conflicts
            .map((c) => `${c.vmid}${c.name ? ` (${c.name})` : ''}`)
            .join(', ')}`
        : ''
    vmidVerify = {
      state: 'error',
      message: `${vmidCheck.message || 'VMID range unavailable'}${conflicts}`,
    }
  }

  let vipVerify
  if (!ha) {
    vipVerify = { state: 'skipped', message: 'Skipped — single control plane' }
  } else {
    const vip =
      mode === 'ipv4' || mode === 'dual-stack' ? String(form.vip || '').trim() : ''
    const vip6 =
      mode === 'ipv6' || mode === 'dual-stack' ? String(form.vip6 || '').trim() : ''
    if (!vip && !vip6) {
      vipVerify = { state: 'idle', message: 'Enter VIP for HA' }
    } else if (vipChecking) {
      vipVerify = { state: 'loading', message: 'Checking VIP on the LAN…' }
    } else if (vipCheck?.ok) {
      vipVerify = { state: 'ok', message: vipCheck.message || 'VIP is available' }
    } else if (vipCheck && !vipCheck.ok) {
      vipVerify = {
        state: 'error',
        message: vipCheck.message || 'VIP is not available',
      }
    } else {
      vipVerify = { state: 'idle', message: 'Waiting for VIP check' }
    }
  }

  function validateStep(i) {
    if (i === 0) {
      if (!form.name.trim()) return 'Name is required'
      if (!form.provider_id) return 'Select a provider'
      if (!String(form.k8s_version || '').trim()) return 'Select a Kubernetes version'
      if (vmidBlocked) return vmidCheck.message || 'Selected VMID range is already in use'
    }
    if (i === 1) {
      if (!String(form.pod_subnet || '').trim()) return 'Pod CIDR is required'
      if (!String(form.service_subnet || '').trim()) return 'Service CIDR is required'
      if (mode === 'dual-stack' || mode === 'ipv6') {
        if (!String(form.pod_subnet_ipv6 || '').trim()) return 'Pod IPv6 CIDR is required'
        if (!String(form.service_subnet_ipv6 || '').trim()) return 'Service IPv6 CIDR is required'
      }
      if (ha) {
        if ((mode === 'ipv4' || mode === 'dual-stack') && !String(form.vip || '').trim()) {
          return 'IPv4 VIP is required when controlplanes > 1'
        }
        if ((mode === 'ipv6' || mode === 'dual-stack') && !String(form.vip6 || '').trim()) {
          return 'IPv6 VIP is required for this network mode when controlplanes > 1'
        }
        if (vipBlocked) return vipCheck.message || 'Selected VIP is not available'
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
    setStep((s) => Math.min(s + 1, STEPS.length - 1))
  }

  function back() {
    setError('')
    setStep((s) => Math.max(s - 1, 0))
  }

  async function submit() {
    setError('')
    for (let i = 0; i < STEPS.length - 1; i++) {
      const e = validateStep(i)
      if (e) {
        setError(e)
        setStep(i)
        return
      }
    }
    if (vmidBlocked) {
      setError(vmidCheck.message || 'Selected VMID range is already in use')
      return
    }
    if (vipBlocked) {
      setError(vipCheck.message || 'Selected VIP is not available')
      return
    }

    const body = {
      name: form.name,
      provider_id: form.provider_id,
      controlplanes: Number(form.controlplanes),
      workers: Number(form.workers),
      network_mode: form.network_mode,
      arch: form.arch,
      vip: ha && mode !== 'ipv6' ? (form.vip || null) : null,
      vip6: ha && mode !== 'ipv4' ? (form.vip6 || null) : null,
      cni: form.cni,
      k8s_version: form.k8s_version,
      max_pods: Number(form.max_pods),
      pod_subnet: String(form.pod_subnet || '').trim(),
      service_subnet: String(form.service_subnet || '').trim(),
      pod_subnet_ipv6:
        form.network_mode === 'dual-stack' || form.network_mode === 'ipv6'
          ? String(form.pod_subnet_ipv6 || '').trim() || null
          : null,
      service_subnet_ipv6:
        form.network_mode === 'dual-stack' || form.network_mode === 'ipv6'
          ? String(form.service_subnet_ipv6 || '').trim() || null
          : null,
      cp_memory: Number(form.cp_memory),
      cp_cores: Number(form.cp_cores),
      cp_disk_gb: Number(form.cp_disk_gb),
      worker_memory: Number(form.worker_memory),
      worker_cores: Number(form.worker_cores),
      worker_disk_gb: Number(form.worker_disk_gb),
      cp_vmid: Number(form.cp_vmid),
    }
    setSaving(true)
    try {
      const res = await api('/clusters', { method: 'POST', body })
      onCreated?.(res)
      onClose?.()
      nav(`/clusters/${res.id}?tab=overview`)
    } catch (err) {
      setError(err.message)
    } finally {
      setSaving(false)
    }
  }

  const createDisabled = saving || verifying || vmidBlocked || vipBlocked
  let createLabel = 'Create cluster'
  let createIcon = <Icon name="play" size={16} />
  if (saving) {
    createLabel = 'Creating…'
    createIcon = <span className="spinner spinner-btn" aria-hidden />
  } else if (verifying) {
    createLabel = 'Verifying…'
    createIcon = <span className="spinner spinner-btn" aria-hidden />
  }

  return (
    <WizardModal
      open={open}
      title="Create cluster"
      icon="plus"
      onClose={onClose}
      steps={STEPS}
      stepIndex={step}
      onStepChange={(i) => {
        setError('')
        setStep(i)
      }}
      footer={
        <>
          <button type="button" className="secondary" onClick={step === 0 ? onClose : back} disabled={saving}>
            {step === 0 ? 'Cancel' : 'Back'}
          </button>
          <div className="wizard-footer-right">
            {step < STEPS.length - 1 ? (
              <button type="button" onClick={next} disabled={saving || (step === 0 && vmidBlocked) || (step === 1 && vipBlocked)}>
                Next
              </button>
            ) : (
              <button type="button" className="btn-icon" onClick={submit} disabled={createDisabled}>
                {createIcon}
                {createLabel}
              </button>
            )}
          </div>
        </>
      }
    >
      {error && <div className="error">{error}</div>}

      {step === 0 && (
        <>
          <p className="wizard-section-title">General</p>
          <div className="form-grid">
            <div className="field">
              <label>Name</label>
              <input value={form.name} onChange={(e) => set('name', e.target.value)} autoFocus />
            </div>
            <div className="field">
              <label>Provider</label>
              <select value={form.provider_id} onChange={(e) => set('provider_id', e.target.value)}>
                <option value="">Select…</option>
                {providers.map((p) => (
                  <option key={p.id} value={p.id}>{p.name}</option>
                ))}
              </select>
            </div>
            <div className="field">
              <label>Control planes (M)</label>
              <input type="number" min={1} value={form.controlplanes} onChange={(e) => set('controlplanes', e.target.value)} />
            </div>
            <div className="field">
              <label>Workers (N)</label>
              <input type="number" min={0} value={form.workers} onChange={(e) => set('workers', e.target.value)} />
            </div>
            <div className="field">
              <label>Guest arch</label>
              <select value={form.arch} onChange={(e) => set('arch', e.target.value)}>
                <option value="amd64">amd64 (x86_64)</option>
                <option value="arm64">arm64 (aarch64)</option>
              </select>
              <p className="hint muted">Defaults from the provider; override per cluster if needed.</p>
            </div>
            <div className="field">
              <label>CNI</label>
              <select value={form.cni} onChange={(e) => set('cni', e.target.value)}>
                <option value="cilium">cilium (default)</option>
                <option value="flannel">flannel</option>
                <option value="calico">calico</option>
              </select>
              <p className="hint muted">Cilium installs first after the apiserver is up (needs helm on mgmt).</p>
            </div>
            <div className="field">
              <label>K8s version</label>
              <K8sVersionSelect
                value={form.k8s_version}
                onChange={(v) => set('k8s_version', v)}
                preferImage
              />
            </div>
            <div className="field">
              <label>Max pods (kubelet)</label>
              <input
                type="number"
                min={1}
                max={1000}
                value={form.max_pods}
                onChange={(e) => set('max_pods', e.target.value)}
              />
            </div>
            <div className="field">
              <label>Base VMID</label>
              <input type="number" value={form.cp_vmid} onChange={(e) => set('cp_vmid', e.target.value)} />
              <p className="hint muted">
                Suggested after existing clusters so VMIDs (and DHCP leases) do not collide across labs.
              </p>
              {!vmidChecking && vmidCheck?.ok && (
                <p className="hint muted">
                  VMIDs {vmidCheck.range_start}–{vmidCheck.range_end} are free on {vmidCheck.node}.
                </p>
              )}
              {!vmidChecking && vmidBlocked && (
                <p className="hint" style={{ color: 'var(--danger, #b91c1c)' }}>
                  {vmidCheck.message}
                </p>
              )}
            </div>
          </div>
          {ha && (
            <p className="hint">
              HA mode (M&gt;1): stacked etcd + kube-vip — VIP required for the selected stack.
              Use an address <strong>outside your DHCP pool</strong> (not leased to any VM).
            </p>
          )}
        </>
      )}

      {step === 1 && (
        <>
          <p className="wizard-section-title">Network</p>
          <div className="segment" role="radiogroup" aria-label="Network mode">
            {[
              { id: 'ipv4', label: 'IPv4' },
              { id: 'ipv6', label: 'IPv6' },
              { id: 'dual-stack', label: 'Dual-stack' },
            ].map((opt) => (
              <button
                key={opt.id}
                type="button"
                className={mode === opt.id ? 'active' : ''}
                onClick={() => set('network_mode', opt.id)}
                aria-pressed={mode === opt.id}
              >
                {opt.label}
              </button>
            ))}
          </div>
          <div className="form-grid" style={{ marginTop: '1rem' }}>
            {ha && (mode === 'ipv4' || mode === 'dual-stack') && (
              <div className="field">
                <label>IPv4 VIP (required)</label>
                <input
                  value={form.vip}
                  onChange={(e) => set('vip', e.target.value)}
                  placeholder="10.1.1.250"
                />
              </div>
            )}
            {ha && (mode === 'ipv6' || mode === 'dual-stack') && (
              <div className="field">
                <label>IPv6 VIP (required)</label>
                <input
                  value={form.vip6}
                  onChange={(e) => set('vip6', e.target.value)}
                  placeholder="fd00:1::200"
                />
              </div>
            )}
            <div className="field">
              <label>Pod subnet (IPv4)</label>
              <input
                value={form.pod_subnet}
                onChange={(e) => set('pod_subnet', e.target.value)}
                placeholder="10.244.0.0/16"
              />
            </div>
            {(mode === 'dual-stack' || mode === 'ipv6') && (
              <div className="field">
                <label>Pod subnet (IPv6)</label>
                <input
                  value={form.pod_subnet_ipv6}
                  onChange={(e) => set('pod_subnet_ipv6', e.target.value)}
                  placeholder="2001:db8:10:0::/56"
                />
              </div>
            )}
            <div className="field">
              <label>Service subnet (IPv4)</label>
              <input
                value={form.service_subnet}
                onChange={(e) => set('service_subnet', e.target.value)}
                placeholder="10.96.0.0/12"
              />
            </div>
            {(mode === 'dual-stack' || mode === 'ipv6') && (
              <div className="field">
                <label>Service subnet (IPv6)</label>
                <input
                  value={form.service_subnet_ipv6}
                  onChange={(e) => set('service_subnet_ipv6', e.target.value)}
                  placeholder="2001:db8:96:1::/112"
                />
              </div>
            )}
          </div>
          {ha && !vipChecking && vipBlocked && (
            <p className="hint" style={{ color: 'var(--danger, #b91c1c)' }}>
              {vipCheck.message}
            </p>
          )}
        </>
      )}

      {step === 2 && (
        <>
          <p className="wizard-section-title">VM size</p>
          <div className="wizard-panel-grid">
            <div>
              <h3 className="card-title" style={{ fontSize: '0.95rem' }}>
                <Icon name="cpu" size={16} /> Control plane ({form.controlplanes})
              </h3>
              <div className="field">
                <label>Memory (MB)</label>
                <input type="number" min={1024} step={512} value={form.cp_memory} onChange={(e) => set('cp_memory', e.target.value)} />
              </div>
              <div className="field">
                <label>vCPUs</label>
                <input type="number" min={1} value={form.cp_cores} onChange={(e) => set('cp_cores', e.target.value)} />
              </div>
              <div className="field">
                <label>Disk (GiB)</label>
                <input type="number" min={10} value={form.cp_disk_gb} onChange={(e) => set('cp_disk_gb', e.target.value)} />
              </div>
            </div>
            <div>
              <h3 className="card-title" style={{ fontSize: '0.95rem' }}>
                <Icon name="worker" size={16} /> Workers ({form.workers})
              </h3>
              <div className="field">
                <label>Memory (MB)</label>
                <input type="number" min={1024} step={512} value={form.worker_memory} onChange={(e) => set('worker_memory', e.target.value)} />
              </div>
              <div className="field">
                <label>vCPUs</label>
                <input type="number" min={1} value={form.worker_cores} onChange={(e) => set('worker_cores', e.target.value)} />
              </div>
              <div className="field">
                <label>Disk (GiB)</label>
                <input type="number" min={10} value={form.worker_disk_gb} onChange={(e) => set('worker_disk_gb', e.target.value)} />
              </div>
            </div>
          </div>
        </>
      )}

      {step === 3 && (
        <>
          <p className="wizard-section-title">Verification</p>
          <ul className="verify-list">
            <VerifyRow state={providerVerify.state} label="Provider" message={providerVerify.message} />
            <VerifyRow state={k8sVerify.state} label="K8s version" message={k8sVerify.message} />
            <VerifyRow state={vmidVerify.state} label="VMID range" message={vmidVerify.message} />
            <VerifyRow state={vipVerify.state} label="VIP" message={vipVerify.message} />
          </ul>
        </>
      )}
    </WizardModal>
  )
}
