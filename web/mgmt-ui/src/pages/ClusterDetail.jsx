import { useCallback, useEffect, useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router-dom'
import { api, getToken } from '../api'

export default function ClusterDetail() {
  const { id } = useParams()
  const nav = useNavigate()
  const [data, setData] = useState(null)
  const [jobs, setJobs] = useState([])
  const [log, setLog] = useState('')
  const [error, setError] = useState('')
  const [upgradeVer, setUpgradeVer] = useState('v1.36.3')
  const [configYaml, setConfigYaml] = useState('# machine config yaml\n')

  const load = useCallback(() => {
    api(`/clusters/${id}`)
      .then(setData)
      .catch((e) => setError(e.message))
    api(`/clusters/${id}/jobs`)
      .then(async (j) => {
        setJobs(j)
        if (j[0]) {
          const text = await api(`/jobs/${j[0].id}/log`).catch(() => '')
          setLog(text)
        }
      })
      .catch(() => {})
  }, [id])

  useEffect(() => {
    load()
    const t = setInterval(load, 4000)
    return () => clearInterval(t)
  }, [load])

  async function del() {
    if (!confirm('Delete cluster and Proxmox VMs?')) return
    await api(`/clusters/${id}`, { method: 'DELETE' })
    nav('/clusters')
  }

  async function addNode(role) {
    await api(`/clusters/${id}/nodes`, { method: 'POST', body: { role } })
    load()
  }

  async function removeNode(nid) {
    if (!confirm('Remove node?')) return
    await api(`/clusters/${id}/nodes/${nid}`, { method: 'DELETE' })
    load()
  }

  async function upgrade() {
    await api(`/clusters/${id}/upgrade`, { method: 'POST', body: { version: upgradeVer } })
    load()
  }

  async function applyConfig() {
    await api(`/clusters/${id}/config`, { method: 'POST', body: { config_yaml: configYaml } })
    load()
  }

  async function downloadKc() {
    setError('')
    try {
      const res = await fetch(`/api/clusters/${id}/kubeconfig`, {
        headers: { Authorization: `Bearer ${getToken()}` },
      })
      if (!res.ok) {
        const body = await res.json().catch(() => ({ error: res.statusText }))
        throw new Error(body.error || res.statusText)
      }
      const text = await res.text()
      const blob = new Blob([text], { type: 'application/yaml' })
      const a = document.createElement('a')
      a.href = URL.createObjectURL(blob)
      a.download = `${data?.cluster?.name || 'cluster'}-admin.conf`
      a.click()
      URL.revokeObjectURL(a.href)
    } catch (err) {
      setError(err.message)
    }
  }

  if (!data) return <p className="muted">{error || 'Loading…'}</p>
  const c = data.cluster
  const nodes = data.nodes || []

  return (
    <div>
      <div className="page-head">
        <h1>{c.name}</h1>
        <div className="row-actions">
          <Link className="btn secondary" to="/clusters">Back</Link>
          <button type="button" className="secondary" onClick={downloadKc}>Kubeconfig</button>
          <button type="button" className="danger" onClick={del}>Delete</button>
        </div>
      </div>
      {error && <div className="error">{error}</div>}
      <div className="grid-stats">
        <div className="stat"><div className="label">Status</div><div className="value" style={{ fontSize: '1.1rem' }}><span className={`badge ${c.status}`}>{c.status}</span></div></div>
        <div className="stat"><div className="label">Topology</div><div className="value" style={{ fontSize: '1.1rem' }}>{c.controlplanes} CP / {c.workers} WK</div></div>
        <div className="stat"><div className="label">VIP</div><div className="value" style={{ fontSize: '1.1rem' }}>{c.vip || '—'}</div></div>
        <div className="stat"><div className="label">CNI</div><div className="value" style={{ fontSize: '1.1rem' }}>{c.cni}</div></div>
      </div>
      {c.error && <div className="card error">{c.error}</div>}

      <div className="card">
        <div className="page-head" style={{ marginBottom: '0.5rem' }}>
          <h2>Nodes</h2>
          <div className="row-actions">
            <button type="button" className="secondary" onClick={() => addNode('worker')}>Add worker</button>
            <button type="button" className="secondary" onClick={() => addNode('controlplane')}>Add CP</button>
          </div>
        </div>
        <table>
          <thead>
            <tr><th>Name</th><th>Role</th><th>VMID</th><th>IP</th><th>Status</th><th></th></tr>
          </thead>
          <tbody>
            {nodes.map((n) => (
              <tr key={n.id}>
                <td>{n.name}</td>
                <td>{n.role}</td>
                <td>{n.vmid ?? '—'}</td>
                <td>{n.ip || '—'}</td>
                <td><span className={`badge ${n.status}`}>{n.status}</span></td>
                <td><button type="button" className="danger" onClick={() => removeNode(n.id)}>Remove</button></td>
              </tr>
            ))}
          </tbody>
        </table>
        {nodes.length === 0 && <p className="muted">Nodes appear after create job finishes.</p>}
      </div>

      <div className="card">
        <h2>Upgrade</h2>
        <div className="row-actions">
          <input style={{ maxWidth: 200 }} value={upgradeVer} onChange={(e) => setUpgradeVer(e.target.value)} />
          <button type="button" onClick={upgrade}>Rolling upgrade</button>
        </div>
      </div>

      <div className="card">
        <h2>Apply machine config</h2>
        <textarea rows={6} value={configYaml} onChange={(e) => setConfigYaml(e.target.value)} />
        <p style={{ marginTop: '0.75rem' }}><button type="button" onClick={applyConfig}>Apply to all nodes</button></p>
      </div>

      <div className="card">
        <h2>Jobs</h2>
        <table>
          <thead><tr><th>Kind</th><th>Status</th><th>Updated</th></tr></thead>
          <tbody>
            {jobs.map((j) => (
              <tr key={j.id}>
                <td>{j.kind}</td>
                <td><span className={`badge ${j.status}`}>{j.status}</span></td>
                <td className="muted">{j.updated_at}</td>
              </tr>
            ))}
          </tbody>
        </table>
        <h3 style={{ marginTop: '1rem' }}>Latest log</h3>
        <pre className="log-box mono">{log || '(empty)'}</pre>
      </div>
    </div>
  )
}
