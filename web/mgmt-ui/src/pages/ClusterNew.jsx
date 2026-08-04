import { useEffect, useState } from 'react'
import { Link, useNavigate } from 'react-router-dom'
import { api } from '../api'

export default function ClusterNew() {
  const nav = useNavigate()
  const [providers, setProviders] = useState([])
  const [error, setError] = useState('')
  const [form, setForm] = useState({
    name: 'lab-ha',
    provider_id: '',
    controlplanes: 3,
    workers: 2,
    vip: '10.1.1.200',
    vip6: '',
    cni: 'cilium',
    k8s_version: 'v1.36.3',
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
      if (p[0]) setForm((f) => ({ ...f, provider_id: p[0].id }))
    })
  }, [])

  function set(k, v) {
    setForm((f) => ({ ...f, [k]: v }))
  }

  async function submit(e) {
    e.preventDefault()
    setError('')
    const body = {
      ...form,
      controlplanes: Number(form.controlplanes),
      workers: Number(form.workers),
      cp_memory: Number(form.cp_memory),
      cp_cores: Number(form.cp_cores),
      cp_disk_gb: Number(form.cp_disk_gb),
      worker_memory: Number(form.worker_memory),
      worker_cores: Number(form.worker_cores),
      worker_disk_gb: Number(form.worker_disk_gb),
      cp_vmid: Number(form.cp_vmid),
      vip: form.vip || null,
      vip6: form.vip6 || null,
    }
    try {
      const res = await api('/clusters', { method: 'POST', body })
      nav(`/clusters/${res.id}`)
    } catch (err) {
      setError(err.message)
    }
  }

  return (
    <div>
      <div className="page-head">
        <h1>Create cluster</h1>
        <Link className="btn secondary" to="/clusters">Cancel</Link>
      </div>
      {error && <div className="error">{error}</div>}
      <form className="card" onSubmit={submit}>
        <p className="muted">Same topology as <code>proxmox-lab-up.sh --controlplanes M --vip … --workers N</code>.</p>
        <div className="form-grid">
          <div className="field"><label>Name</label><input value={form.name} onChange={(e) => set('name', e.target.value)} required /></div>
          <div className="field">
            <label>Provider</label>
            <select value={form.provider_id} onChange={(e) => set('provider_id', e.target.value)} required>
              <option value="">Select…</option>
              {providers.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
            </select>
          </div>
          <div className="field"><label>Control planes (M)</label><input type="number" min={1} value={form.controlplanes} onChange={(e) => set('controlplanes', e.target.value)} /></div>
          <div className="field"><label>Workers (N)</label><input type="number" min={0} value={form.workers} onChange={(e) => set('workers', e.target.value)} /></div>
          <div className="field"><label>VIP (required if M&gt;1)</label><input value={form.vip} onChange={(e) => set('vip', e.target.value)} placeholder="10.1.1.200" /></div>
          <div className="field"><label>VIP6 (optional dual-stack)</label><input value={form.vip6} onChange={(e) => set('vip6', e.target.value)} /></div>
          <div className="field">
            <label>CNI</label>
            <select value={form.cni} onChange={(e) => set('cni', e.target.value)}>
              <option value="cilium">cilium</option>
              <option value="calico">calico</option>
              <option value="flannel">flannel</option>
            </select>
          </div>
          <div className="field"><label>K8s version</label><input value={form.k8s_version} onChange={(e) => set('k8s_version', e.target.value)} /></div>
          <div className="field"><label>CP memory (MB)</label><input type="number" value={form.cp_memory} onChange={(e) => set('cp_memory', e.target.value)} /></div>
          <div className="field"><label>CP cores</label><input type="number" value={form.cp_cores} onChange={(e) => set('cp_cores', e.target.value)} /></div>
          <div className="field"><label>CP disk (GiB)</label><input type="number" value={form.cp_disk_gb} onChange={(e) => set('cp_disk_gb', e.target.value)} /></div>
          <div className="field"><label>Worker memory (MB)</label><input type="number" value={form.worker_memory} onChange={(e) => set('worker_memory', e.target.value)} /></div>
          <div className="field"><label>Worker cores</label><input type="number" value={form.worker_cores} onChange={(e) => set('worker_cores', e.target.value)} /></div>
          <div className="field"><label>Worker disk (GiB)</label><input type="number" value={form.worker_disk_gb} onChange={(e) => set('worker_disk_gb', e.target.value)} /></div>
          <div className="field"><label>Base VMID</label><input type="number" value={form.cp_vmid} onChange={(e) => set('cp_vmid', e.target.value)} /></div>
        </div>
        <button type="submit">Create</button>
      </form>
    </div>
  )
}
