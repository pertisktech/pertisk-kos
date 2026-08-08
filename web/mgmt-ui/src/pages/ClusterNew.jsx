import { useEffect, useState } from 'react'
import { Link, useNavigate } from 'react-router-dom'
import { api } from '../api'
import { Icon } from '../components/Icons'
import K8sVersionSelect from '../components/K8sVersionSelect'

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

export default function ClusterNew() {
  const nav = useNavigate()
  const [providers, setProviders] = useState([])
  const [error, setError] = useState('')
  const [saving, setSaving] = useState(false)
  const [vmidCheck, setVmidCheck] = useState(null)
  const [vmidChecking, setVmidChecking] = useState(false)
  const [vipCheck, setVipCheck] = useState(null)
  const [vipChecking, setVipChecking] = useState(false)
  const [form, setForm] = useState({
    name: 'lab-ha',
    provider_id: '',
    controlplanes: 3,
    workers: 2,
    network_mode: 'ipv4', // ipv4 | ipv6 | dual-stack
    arch: 'amd64', // amd64 | arm64
    vip: '10.1.1.200',
    vip6: 'fd00:1::200',
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
  })

  useEffect(() => {
    api('/providers').then((p) => {
      setProviders(p)
      if (p[0]) {
        const arch = p[0].arch === 'arm64' ? 'arm64' : 'amd64'
        setForm((f) => ({ ...f, provider_id: p[0].id, arch }))
      }
    })
  }, [])

  // Live VMID conflict check against the provider node.
  useEffect(() => {
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
  }, [form.provider_id, form.cp_vmid, form.controlplanes, form.workers])

  // Live VIP free check (ping / :6443 / other clusters) when HA.
  useEffect(() => {
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
  }, [form.controlplanes, form.network_mode, form.vip, form.vip6])

  function set(k, v) {
    setForm((f) => {
      const next = { ...f, [k]: v }
      // VIP is HA-only — clear when dropping to a single control plane.
      if (k === 'controlplanes' && Number(v) <= 1) {
        next.vip = ''
        next.vip6 = ''
      }
      // Inherit guest arch from the selected provider.
      if (k === 'provider_id') {
        const p = providers.find((x) => x.id === v)
        if (p) next.arch = p.arch === 'arm64' ? 'arm64' : 'amd64'
      }
      return next
    })
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

  async function submit(e) {
    e.preventDefault()
    setError('')

    if (ha) {
      if ((mode === 'ipv4' || mode === 'dual-stack') && !String(form.vip || '').trim()) {
        setError('IPv4 VIP is required when controlplanes > 1')
        return
      }
      if ((mode === 'ipv6' || mode === 'dual-stack') && !String(form.vip6 || '').trim()) {
        setError('IPv6 VIP is required for this network mode when controlplanes > 1')
        return
      }
    }
    if (!String(form.k8s_version || '').trim()) {
      setError('Select a Kubernetes version')
      return
    }
    if (!String(form.pod_subnet || '').trim()) {
      setError('Pod CIDR is required')
      return
    }
    if (!String(form.service_subnet || '').trim()) {
      setError('Service CIDR is required')
      return
    }
    if (mode === 'dual-stack' || mode === 'ipv6') {
      if (!String(form.pod_subnet_ipv6 || '').trim()) {
        setError('Pod IPv6 CIDR is required for dual-stack / IPv6')
        return
      }
      if (!String(form.service_subnet_ipv6 || '').trim()) {
        setError('Service IPv6 CIDR is required for dual-stack / IPv6')
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
      // Single-CP talks to the node IP — never store / pass a VIP.
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
    <div>
      <div className="page-head">
        <h1>
          <Icon name="plus" size={22} /> Create cluster
        </h1>
        <Link className="btn secondary btn-icon" to="/clusters">
          <Icon name="back" size={16} /> Cancel
        </Link>
      </div>
      {error && <div className="error">{error}</div>}

      <form onSubmit={submit}>
        <section className="card">
          <h2 className="card-title">
            <Icon name="clusters" size={18} /> General
          </h2>
          <div className="form-grid">
            <div className="field">
              <label>Name</label>
              <input value={form.name} onChange={(e) => set('name', e.target.value)} required />
            </div>
            <div className="field">
              <label>Provider</label>
              <select value={form.provider_id} onChange={(e) => set('provider_id', e.target.value)} required>
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
              <p className="hint muted">
                Defaults from the provider; override per cluster if needed.
                Uses <code>pertisk-cloud-{'{arch}'}.qcow2</code>. arm64 needs Proxmox root SSH.
              </p>
            </div>
            <div className="field">
              <label>CNI</label>
              <select value={form.cni} onChange={(e) => set('cni', e.target.value)}>
                <option value="cilium">cilium</option>
                <option value="calico">calico</option>
                <option value="flannel">flannel</option>
              </select>
            </div>
            <div className="field">
              <label>K8s version</label>
              <K8sVersionSelect
                value={form.k8s_version}
                onChange={(v) => set('k8s_version', v)}
                preferImage
              />
              <p className="hint muted">
                Latest 10 stable releases. Picking a version other than the image pin rebuilds the cloud image.
              </p>
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
              <p className="hint muted">
                Written as <code>machine.kubelet.extraConfig.maxPods</code> (Kubernetes default is 110).
              </p>
            </div>
            <div className="field">
              <label>Base VMID</label>
              <input type="number" value={form.cp_vmid} onChange={(e) => set('cp_vmid', e.target.value)} />
              {providers.find((p) => p.id === form.provider_id)?.kind === 'vsphere' ? (
                <p className="hint muted">
                  Inventory IDs only (cp={form.cp_vmid}, then +1…). ESXi Host Client URLs use a different
                  MoRef (e.g. <code>/ui/#/host/vms/31</code>) — that is not the Base VMID.
                </p>
              ) : (
                <p className="hint muted">First control-plane QEMU ID on Proxmox; workers follow sequentially.</p>
              )}
              {!vmidChecking && vmidCheck?.ok && (
                <p className="hint muted">
                  VMIDs {vmidCheck.range_start}–{vmidCheck.range_end} are free on {vmidCheck.node}.
                </p>
              )}
              {!vmidChecking && vmidBlocked && (
                <p className="hint" style={{ color: 'var(--danger, #b91c1c)' }}>
                  {vmidCheck.message}
                  {vmidCheck.conflicts?.length > 0 && (
                    <>
                      {' '}
                      In use:{' '}
                      {vmidCheck.conflicts
                        .map((c) => `${c.vmid}${c.name ? ` (${c.name})` : ''}`)
                        .join(', ')}
                    </>
                  )}
                </p>
              )}
            </div>
          </div>
          {ha && (
            <p className="hint">
              HA mode (M&gt;1): stacked etcd + kube-vip — VIP required for the selected stack.
            </p>
          )}
        </section>

        <section className="card">
          <h2 className="card-title">
            <Icon name="network" size={18} /> Network
          </h2>
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
                  placeholder="10.1.1.200"
                  required
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
                  required
                />
              </div>
            )}
          </div>

          <div className="form-grid" style={{ marginTop: '1rem' }}>
            <div className="field">
              <label>Pod subnet (IPv4)</label>
              <input
                value={form.pod_subnet}
                onChange={(e) => set('pod_subnet', e.target.value)}
                placeholder="10.244.0.0/16"
                required
              />
            </div>
            {(mode === 'dual-stack' || mode === 'ipv6') ? (
              <div className="field">
                <label>Pod subnet (IPv6)</label>
                <input
                  value={form.pod_subnet_ipv6}
                  onChange={(e) => set('pod_subnet_ipv6', e.target.value)}
                  placeholder="2001:db8:10:0::/56"
                  required
                />
              </div>
            ) : (
              <div className="field" />
            )}
            <div className="field">
              <label>Service subnet (IPv4)</label>
              <input
                value={form.service_subnet}
                onChange={(e) => set('service_subnet', e.target.value)}
                placeholder="10.96.0.0/12"
                required
              />
            </div>
            {(mode === 'dual-stack' || mode === 'ipv6') ? (
              <div className="field">
                <label>Service subnet (IPv6)</label>
                <input
                  value={form.service_subnet_ipv6}
                  onChange={(e) => set('service_subnet_ipv6', e.target.value)}
                  placeholder="2001:db8:96:1::/112"
                  required
                />
              </div>
            ) : (
              <div className="field" />
            )}
          </div>
          <p className="hint muted" style={{ marginTop: '0.5rem' }}>
            {mode === 'dual-stack' || mode === 'ipv6'
              ? 'Written as cluster.network.podSubnets / serviceSubnets (IPv4 + IPv6).'
              : 'Written as cluster.network.podSubnets / serviceSubnets (IPv4 only).'}
          </p>
          {ha && !vipChecking && vipCheck?.ok && (
            <p className="hint muted">{vipCheck.message}</p>
          )}
          {ha && !vipChecking && vipBlocked && (
            <p className="hint" style={{ color: 'var(--danger, #b91c1c)' }}>
              {vipCheck.message}
            </p>
          )}
          <p className="hint muted">
            {!ha && 'Single control plane: kubeconfig uses the CP node IP (no kube-vip).'}
            {ha && mode === 'ipv4' && 'HA: kube-vip ARP VIP is the API endpoint — must be free on the LAN.'}
            {ha && mode === 'ipv6' && 'HA: IPv6 VIP is the API endpoint — must be free on the LAN.'}
            {ha && mode === 'dual-stack' && 'HA dual-stack: both VIPs must be free (ping / :6443 / other clusters).'}
          </p>
        </section>

        <div className="panel-grid">
          <section className="card panel">
            <h2 className="card-title">
              <Icon name="cpu" size={18} /> Control plane
            </h2>
            <p className="muted panel-sub">{form.controlplanes} VM{Number(form.controlplanes) === 1 ? '' : 's'}</p>
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
          </section>

          <section className="card panel">
            <h2 className="card-title">
              <Icon name="worker" size={18} /> Workers
            </h2>
            <p className="muted panel-sub">{form.workers} VM{Number(form.workers) === 1 ? '' : 's'}</p>
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
          </section>
        </div>

        <section className="card verify-panel" aria-label="Verification">
          <h2 className="card-title">
            <Icon name="check" size={18} /> Verification
          </h2>
          <ul className="verify-list">
            <VerifyRow state={providerVerify.state} label="Provider" message={providerVerify.message} />
            <VerifyRow state={k8sVerify.state} label="K8s version" message={k8sVerify.message} />
            <VerifyRow state={vmidVerify.state} label="VMID range" message={vmidVerify.message} />
            <VerifyRow state={vipVerify.state} label="VIP" message={vipVerify.message} />
          </ul>
        </section>

        <div className="form-footer">
          <button type="submit" className="btn-icon" disabled={createDisabled}>
            {createIcon}
            {createLabel}
          </button>
        </div>
      </form>
    </div>
  )
}
