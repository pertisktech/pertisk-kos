import { useCallback, useEffect, useRef, useState } from 'react'
import { Link, useNavigate, useParams, useSearchParams } from 'react-router-dom'
import { api, getToken } from '../api'
import { Icon } from '../components/Icons'
import { useConfirm } from '../components/Confirm'
import Checkbox from '../components/Checkbox'
import Modal from '../components/Modal'
import K8sVersionSelect from '../components/K8sVersionSelect'

const TABS = [
  { id: 'overview', label: 'Overview', icon: 'dashboard' },
  { id: 'nodes', label: 'Nodes', icon: 'worker' },
  { id: 'config', label: 'Config', icon: 'edit' },
  { id: 'upgrade', label: 'Upgrade', icon: 'play' },
  { id: 'jobs', label: 'Jobs', icon: 'providers' },
]

function NodeAddresses({ node, dualStack }) {
  const v4 = node.ip?.trim()
  const v6 = node.ip6?.trim()
  if (!v4 && !v6) return <span className="muted">—</span>
  return (
    <div className="node-ips">
      {v4 && <div className="mono-inline">{v4}</div>}
      {dualStack && (
        v6
          ? <div className="mono-inline node-ip6">{v6}</div>
          : <div className="muted node-ip6">—</div>
      )}
      {!dualStack && v6 && <div className="mono-inline node-ip6">{v6}</div>}
    </div>
  )
}

function formatHw(node) {
  const cores = node.cores ?? '—'
  const mem = node.memory != null ? `${node.memory} MB` : '—'
  const disk = node.disk_gb != null ? `${node.disk_gb} GiB` : '—'
  return `${cores} vCPU · ${mem} · ${disk}`
}

function NodesTable({
  nodes,
  clusterId,
  dualStack,
  showK8s = true,
  showHw = false,
  targetVersion,
  selectable = false,
  selected = new Set(),
  onToggle,
  onToggleAll,
  onHardware,
  onReboot,
}) {
  const allSelected = selectable && nodes.length > 0 && nodes.every((n) => selected.has(n.id))
  const someSelected = selectable && nodes.some((n) => selected.has(n.id)) && !allSelected

  return (
    <table>
      <thead>
        <tr>
          {selectable && (
            <th className="col-check">
              <Checkbox
                id="nodes-select-all"
                checked={allSelected}
                indeterminate={someSelected}
                onChange={(on) => onToggleAll?.(on)}
              />
            </th>
          )}
          <th>Name</th>
          <th>Role</th>
          <th>VMID</th>
          <th>{dualStack ? 'IPv4 / IPv6' : 'IP'}</th>
          {showK8s && <th>K8s</th>}
          {showHw && <th>Hardware</th>}
          <th>Status</th>
          {(onHardware || onReboot) && <th className="col-actions" />}
        </tr>
      </thead>
      <tbody>
        {nodes.map((n) => {
          const atTarget = targetVersion && n.k8s_version === targetVersion
          const upgrading = n.status === 'upgrading'
          const isSel = selected.has(n.id)
          return (
            <tr
              key={n.id}
              className={`${upgrading ? 'row-upgrading' : ''} ${isSel ? 'row-selected' : ''}`}
            >
              {selectable && (
                <td className="col-check">
                  <Checkbox
                    id={`node-${n.id}`}
                    checked={isSel}
                    onChange={() => onToggle?.(n.id)}
                  />
                </td>
              )}
              <td>
                {clusterId ? (
                  <Link className="node-link" to={`/clusters/${clusterId}/nodes/${n.id}`}>
                    {n.name}
                  </Link>
                ) : (
                  n.name
                )}
              </td>
              <td>
                <span className="badge">{n.role === 'controlplane' ? 'CP' : 'worker'}</span>
              </td>
              <td className="muted">{n.vmid ?? '—'}</td>
              <td><NodeAddresses node={n} dualStack={dualStack} /></td>
              {showK8s && (
                <td>
                  <span className={`mono-inline ${atTarget ? 'ver-match' : ''}`}>
                    {n.k8s_version || '—'}
                  </span>
                  {targetVersion && !atTarget && n.k8s_version && (
                    <span className="muted ver-arrow"> → {targetVersion}</span>
                  )}
                </td>
              )}
              {showHw && <td className="hw-cell">{formatHw(n)}</td>}
              <td><span className={`badge ${n.status}`}>{n.status}</span></td>
              {(onHardware || onReboot) && (
                <td className="col-actions">
                  <div className="row-actions-cell">
                    {onReboot && (
                      <button
                        type="button"
                        className="secondary btn-icon"
                        onClick={() => onReboot(n)}
                        title="Reboot guest"
                      >
                        <Icon name="reboot" size={14} />
                      </button>
                    )}
                    {onHardware && (
                      <button
                        type="button"
                        className="secondary btn-icon"
                        onClick={() => onHardware(n)}
                        title="Upgrade hardware"
                      >
                        <Icon name="cpu" size={14} />
                      </button>
                    )}
                  </div>
                </td>
              )}
            </tr>
          )
        })}
      </tbody>
    </table>
  )
}

function defaultsForRole(cluster, role) {
  if (role === 'controlplane') {
    return {
      memory: cluster?.cp_memory ?? 4096,
      cores: cluster?.cp_cores ?? 2,
      disk_gb: cluster?.cp_disk_gb ?? 50,
    }
  }
  return {
    memory: cluster?.worker_memory ?? 8192,
    cores: cluster?.worker_cores ?? 4,
    disk_gb: cluster?.worker_disk_gb ?? 75,
  }
}

export default function ClusterDetail() {
  const { id } = useParams()
  const [search, setSearch] = useSearchParams()
  const nav = useNavigate()
  const confirm = useConfirm()
  const [data, setData] = useState(null)
  const [jobs, setJobs] = useState([])
  const [log, setLog] = useState('')
  const [selectedJob, setSelectedJob] = useState(null)
  const [error, setError] = useState('')
  const [upgradeVer, setUpgradeVer] = useState('')
  const [configYaml, setConfigYaml] = useState(`version: v1alpha1
machine:
  dashboard:
    theme: catppuccin
    border: bordered
`)
  const [followLog, setFollowLog] = useState(true)
  const [selectedNodes, setSelectedNodes] = useState(() => new Set())
  const [addOpen, setAddOpen] = useState(false)
  const [addForm, setAddForm] = useState({
    role: 'worker',
    count: 1,
    memory: 8192,
    cores: 4,
    disk_gb: 75,
  })
  const [hwOpen, setHwOpen] = useState(false)
  const [hwNode, setHwNode] = useState(null)
  const [hwForm, setHwForm] = useState({ memory: 4096, cores: 2, disk_gb: 50 })
  const [busy, setBusy] = useState(false)
  const logRef = useRef(null)
  const selectedJobRef = useRef(null)

  const tab = TABS.some((t) => t.id === search.get('tab'))
    ? search.get('tab')
    : 'overview'

  function setTab(next) {
    setSearch(next === 'overview' ? {} : { tab: next }, { replace: true })
  }

  const load = useCallback(async () => {
    try {
      const d = await api(`/clusters/${id}`)
      setData(d)
      setUpgradeVer((prev) => prev || d?.cluster?.k8s_version || '')
      setSelectedNodes((prev) => {
        const ids = new Set((d?.nodes || []).map((n) => n.id))
        const next = new Set()
        for (const x of prev) if (ids.has(x)) next.add(x)
        return next
      })
    } catch (e) {
      setError(e.message)
    }

    try {
      const j = await api(`/clusters/${id}/jobs`)
      setJobs(j)
      const prev = selectedJobRef.current
      const jobId =
        (prev && j.some((x) => x.id === prev) ? prev : null) || j[0]?.id || null
      if (jobId !== prev) {
        selectedJobRef.current = jobId
        setSelectedJob(jobId)
      }
      if (jobId) {
        const text = await api(`/jobs/${jobId}/log`).catch(() => '')
        setLog(text)
      } else {
        setLog('')
      }
    } catch {
      /* jobs optional while cluster loads */
    }
  }, [id])

  useEffect(() => {
    load()
    const status = data?.cluster?.status
    const busy =
      status === 'provisioning' ||
      status === 'pending' ||
      status === 'upgrading' ||
      status === 'deleting'
    const t = setInterval(load, busy ? 2000 : 4000)
    return () => clearInterval(t)
  }, [load, data?.cluster?.status])

  useEffect(() => {
    if (!followLog || !logRef.current) return
    const el = logRef.current
    el.scrollTop = el.scrollHeight
  }, [log, followLog, tab])

  function onLogScroll() {
    const el = logRef.current
    if (!el) return
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 48
    if (atBottom && !followLog) setFollowLog(true)
    if (!atBottom && followLog) setFollowLog(false)
  }

  async function loadJobLog(jobId) {
    selectedJobRef.current = jobId
    setSelectedJob(jobId)
    const text = await api(`/jobs/${jobId}/log`).catch(() => '')
    setLog(text)
    setFollowLog(true)
  }

  function selectJob(jobId) {
    selectedJobRef.current = jobId
    setSelectedJob(jobId)
  }

  async function del() {
    setError('')
    let check
    try {
      check = await api(`/clusters/${id}/delete-check`)
    } catch (err) {
      setError(err.message)
      return
    }

    const p = check.provider || {}
    const providerLine = p.exists
      ? `Provider: ${p.name} (${p.url} · node ${p.node}) — ${
          p.reachable ? `reachable (Proxmox ${p.version || '?'})` : `unreachable${p.error ? `: ${p.error}` : ''}`
        }`
      : 'Provider: missing — DB-only delete; Proxmox VMs may remain.'

    const ok = await confirm({
      title: 'Delete cluster',
      message: [
        `Remove “${check.cluster_name || data?.cluster?.name || id}”?`,
        providerLine,
        `Planned VMs: ${check.planned_vms} (recorded nodes: ${check.recorded_nodes}).`,
        check.warning || 'VMs on this provider will be destroyed (best-effort).',
      ].join('\n\n'),
      confirmLabel: p.exists ? 'Delete on provider' : 'Delete from DB',
      tone: 'danger',
    })
    if (!ok) return
    try {
      const res = await api(`/clusters/${id}`, { method: 'DELETE' })
      // Async delete leaves a "deleting" row until the job finishes — list polls it off.
      if (res?.job_id) {
        nav(`/clusters?deleting=${id}`)
      } else {
        nav('/clusters')
      }
    } catch (err) {
      setError(err.message)
    }
  }

  function openAddModal() {
    const d = defaultsForRole(data?.cluster, 'worker')
    setAddForm({ role: 'worker', count: 1, ...d })
    setAddOpen(true)
  }

  function setAddRole(role) {
    const d = defaultsForRole(data?.cluster, role)
    setAddForm((f) => ({ ...f, role, ...d }))
  }

  async function submitAdd() {
    setBusy(true)
    setError('')
    try {
      const res = await api(`/clusters/${id}/nodes`, {
        method: 'POST',
        body: {
          role: addForm.role,
          count: Number(addForm.count) || 1,
          memory: Number(addForm.memory),
          cores: Number(addForm.cores),
          disk_gb: Number(addForm.disk_gb),
        },
      })
      setAddOpen(false)
      if (res?.job_id) selectJob(res.job_id)
      setTab('jobs')
      load()
    } catch (err) {
      setError(err.message)
    } finally {
      setBusy(false)
    }
  }

  function toggleNode(nid) {
    setSelectedNodes((prev) => {
      const next = new Set(prev)
      if (next.has(nid)) next.delete(nid)
      else next.add(nid)
      return next
    })
  }

  function toggleAll(on) {
    if (!on) {
      setSelectedNodes(new Set())
      return
    }
    setSelectedNodes(new Set((data?.nodes || []).map((n) => n.id)))
  }

  async function removeSelected() {
    const nodes = data?.nodes || []
    const picked = nodes.filter((n) => selectedNodes.has(n.id))
    if (picked.length === 0) return
    const names = picked.map((n) => n.name).join(', ')
    const ok = await confirm({
      title: picked.length === 1 ? 'Remove node' : `Remove ${picked.length} nodes`,
      message: `Remove and destroy VM(s):\n${names}`,
      confirmLabel: picked.length === 1 ? 'Remove' : `Remove ${picked.length}`,
      tone: 'danger',
    })
    if (!ok) return
    setBusy(true)
    setError('')
    try {
      const res = await api(`/clusters/${id}/nodes/bulk-delete`, {
        method: 'POST',
        body: { node_ids: picked.map((n) => n.id) },
      })
      setSelectedNodes(new Set())
      if (res?.job_id) selectJob(res.job_id)
      setTab('jobs')
      load()
    } catch (err) {
      setError(err.message)
    } finally {
      setBusy(false)
    }
  }

  async function rebootNode(node) {
    const ok = await confirm({
      title: 'Reboot node',
      message: `Reboot “${node.name}”?\n\nThe guest OS will restart via pertiskd.`,
      confirmLabel: 'Reboot',
      tone: 'primary',
    })
    if (!ok) return
    setBusy(true)
    setError('')
    try {
      const res = await api(`/clusters/${id}/nodes/${node.id}/reboot`, { method: 'POST' })
      if (res?.job_id) selectJob(res.job_id)
      setTab('jobs')
      load()
    } catch (err) {
      setError(err.message)
    } finally {
      setBusy(false)
    }
  }

  async function rebootSelected() {
    const nodes = data?.nodes || []
    const picked = nodes.filter((n) => selectedNodes.has(n.id))
    if (picked.length === 0) return
    const names = picked.map((n) => n.name).join(', ')
    const ok = await confirm({
      title: picked.length === 1 ? 'Reboot node' : `Reboot ${picked.length} nodes`,
      message: `Reboot guest OS on:\n${names}`,
      confirmLabel: picked.length === 1 ? 'Reboot' : `Reboot ${picked.length}`,
      tone: 'primary',
    })
    if (!ok) return
    setBusy(true)
    setError('')
    try {
      const res = await api(`/clusters/${id}/nodes/bulk-reboot`, {
        method: 'POST',
        body: { node_ids: picked.map((n) => n.id) },
      })
      setSelectedNodes(new Set())
      if (res?.job_id) selectJob(res.job_id)
      setTab('jobs')
      load()
    } catch (err) {
      setError(err.message)
    } finally {
      setBusy(false)
    }
  }

  function openHardware(node) {
    setHwNode(node)
    setHwForm({
      memory: node.memory ?? defaultsForRole(data?.cluster, node.role).memory,
      cores: node.cores ?? defaultsForRole(data?.cluster, node.role).cores,
      disk_gb: node.disk_gb ?? defaultsForRole(data?.cluster, node.role).disk_gb,
    })
    setHwOpen(true)
  }

  async function submitHardware() {
    if (!hwNode) return
    const curDisk = hwNode.disk_gb ?? defaultsForRole(data?.cluster, hwNode.role).disk_gb
    const curMem = hwNode.memory ?? defaultsForRole(data?.cluster, hwNode.role).memory
    const curCores = hwNode.cores ?? defaultsForRole(data?.cluster, hwNode.role).cores
    const nextMem = Number(hwForm.memory)
    const nextCores = Number(hwForm.cores)
    const nextDisk = Number(hwForm.disk_gb)
    if (nextDisk < curDisk) {
      setError(`Disk can only grow (have ${curDisk} GiB, asked ${nextDisk} GiB)`)
      return
    }
    const body = {
      memory: nextMem,
      cores: nextCores,
      disk_gb: nextDisk,
    }
    const changed = []
    if (nextMem !== Number(curMem)) changed.push(`${nextMem} MB`)
    if (nextCores !== Number(curCores)) changed.push(`${nextCores} vCPU`)
    if (nextDisk !== Number(curDisk)) changed.push(`${nextDisk} GiB disk`)
    // Close form before confirm so dialogs are not stacked.
    const nodeName = hwNode.name
    const nodeId = hwNode.id
    setHwOpen(false)
    setHwNode(null)
    // Always include disk_gb so a Proxmox-only grow can re-run guest EPHEMERAL expand.
    const ok = await confirm({
      title: 'Upgrade hardware',
      message: changed.length
        ? `Resize “${nodeName}” on Proxmox to ${changed.join(' · ')}?\n\nDisk grow expands guest EPHEMERAL (/var). CPU/memory stop-starts the VM.`
        : `Re-apply hardware on “${nodeName}” (${nextCores} vCPU · ${nextMem} MB · ${nextDisk} GiB)?\n\nThis re-runs guest EPHEMERAL grow if Proxmox disk is already larger.`,
      confirmLabel: 'Apply hardware',
      tone: 'primary',
    })
    if (!ok) return
    setBusy(true)
    setError('')
    try {
      const res = await api(`/clusters/${id}/nodes/${nodeId}/hardware`, {
        method: 'PUT',
        body,
      })
      if (res?.job_id) selectJob(res.job_id)
      setTab('jobs')
      load()
    } catch (err) {
      setError(err.message)
    } finally {
      setBusy(false)
    }
  }

  async function upgrade() {
    const ok = await confirm({
      title: 'Rolling upgrade',
      message: `Upgrade all nodes toward ${upgradeVer}?\n\nOrder: control planes one-by-one, then workers.\nEach node: drain → apply version → wait Ready → uncordon.\nHA (M≥3 + VIP) keeps the API up during CP upgrades.`,
      confirmLabel: 'Start upgrade',
      tone: 'primary',
    })
    if (!ok) return
    const res = await api(`/clusters/${id}/upgrade`, { method: 'POST', body: { version: upgradeVer } })
    if (res?.job_id) selectJob(res.job_id)
    setTab('jobs')
    load()
  }

  async function applyConfig() {
    const ok = await confirm({
      title: 'Apply machine config',
      message: 'Apply this YAML to all nodes in the cluster?',
      confirmLabel: 'Apply',
      tone: 'primary',
    })
    if (!ok) return
    const res = await api(`/clusters/${id}/config`, { method: 'POST', body: { config_yaml: configYaml } })
    if (res?.job_id) selectJob(res.job_id)
    setTab('jobs')
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

  if (!data) {
    return (
      <div className="detail-loading">
        <div className="skeleton-line w-40" />
        <div className="skeleton-line w-80" />
        <p className="muted">{error || 'Loading cluster…'}</p>
      </div>
    )
  }

  const c = data.cluster
  const nodes = data.nodes || []
  const netLabel = c.network_mode || (c.vip6 && c.vip ? 'dual-stack' : c.vip6 ? 'ipv6' : 'ipv4')
  const dualStack = netLabel === 'dual-stack' || netLabel === 'ipv6'
  const cps = nodes.filter((n) => n.role === 'controlplane')
  const wks = nodes.filter((n) => n.role !== 'controlplane')
  const latestJob = jobs[0]
  const createJob = jobs.find((j) => j.kind === 'create_cluster')
  const createRunning = createJob?.status === 'running' || createJob?.status === 'queued'
  const createFailed = createJob?.status === 'failed'
  const nodesWithoutIp = nodes.filter((n) => !n.ip?.trim())
  const hollowReady =
    c.status === 'ready' &&
    nodes.length > 0 &&
    nodesWithoutIp.length === nodes.length
  const upgradeRunning = jobs.some((j) => j.kind === 'upgrade_cluster' && j.status === 'running')
  const selCount = selectedNodes.size

  return (
    <div className="detail">
      <div className="detail-top">
      <div className="page-head">
        <div className="detail-title">
          <h1>
            <Icon name="clusters" size={22} /> {c.name}
          </h1>
          <span className={`badge ${c.status}`}>{c.status}</span>
        </div>
        <div className="row-actions">
          <Link className="btn secondary btn-icon" to="/clusters">
            <Icon name="back" size={16} /> Back
          </Link>
          <button type="button" className="secondary btn-icon" onClick={downloadKc}>
            <Icon name="download" size={16} /> Kubeconfig
          </button>
          <button type="button" className="danger btn-icon" onClick={del}>
            <Icon name="trash" size={16} /> Delete
          </button>
        </div>
      </div>

      {error && <div className="error">{error}</div>}
      {c.error && (
        <div className="banner danger">
          <Icon name="alert" size={18} />
          <span>{c.error}</span>
        </div>
      )}
      {createRunning && (
        <div className="banner info">
          <Icon name="play" size={18} />
          <span>
            Creating cluster — Proxmox VMs and join in progress.
            {' '}
            <button type="button" className="linkish" onClick={() => setTab('jobs')}>Watch job log</button>
            {' · '}
            <button type="button" className="linkish" onClick={() => setTab('nodes')}>Nodes</button>
          </span>
        </div>
      )}
      {createFailed && !c.error && (
        <div className="banner danger">
          <Icon name="alert" size={18} />
          <span>
            Create cluster failed{createJob?.error ? `: ${createJob.error}` : '.'}
            {' '}
            <button type="button" className="linkish" onClick={() => setTab('jobs')}>View job log</button>
          </span>
        </div>
      )}
      {hollowReady && (
        <div className="banner warn">
          <Icon name="alert" size={18} />
          <span>
            Cluster shows ready but no node has an IP — create likely used a lab-up stub
            (missing <code className="mono-inline">MGMT_LAB_UP</code>) or VMs never joined.
            Delete this cluster, fix lab-up on the server, and create again.
          </span>
        </div>
      )}

      <div className="grid-stats detail-stats">
        <div className="stat">
          <div className="label">Provider</div>
          <div className="value sm">
            {c.provider_name || <span className="badge error">missing</span>}
          </div>
          {c.provider_node && (
            <div className="muted" style={{ fontSize: '0.75rem', marginTop: 4 }}>
              {c.provider_node}
            </div>
          )}
        </div>
        <div className="stat">
          <div className="label">Topology</div>
          <div className="value sm">{c.controlplanes} CP / {c.workers} WK</div>
        </div>
        <div className="stat">
          <div className="label">Network</div>
          <div className="value sm">{netLabel}</div>
        </div>
        <div className="stat">
          <div className="label">VIP</div>
          <div className="value xs mono-inline">
            {c.vip || '—'}
            {c.vip6 ? <><br />{c.vip6}</> : null}
          </div>
        </div>
        <div className="stat">
          <div className="label">CNI / K8s</div>
          <div className="value sm">{c.cni} · {c.k8s_version}</div>
        </div>
      </div>
      </div>

      <div className="tabs-shell">
        <div className="tabs" role="tablist">
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
              {t.id === 'nodes' && nodes.length > 0 && (
                <span className="tab-count">{nodes.length}</span>
              )}
              {t.id === 'jobs' && jobs.length > 0 && (
                <span className="tab-count">{jobs.length}</span>
              )}
            </button>
          ))}
        </div>

        <div className="tab-panel card" role="tabpanel">
          {tab === 'overview' && (
            <div className="tab-body">
              <div className="overview-grid">
                <section>
                  <h3 className="section-label">Cluster</h3>
                  <dl className="kv">
                    <div><dt>Name</dt><dd>{c.name}</dd></div>
                    <div><dt>Status</dt><dd><span className={`badge ${c.status}`}>{c.status}</span></dd></div>
                    <div><dt>Endpoint</dt><dd className="mono-inline">{c.endpoint || '—'}</dd></div>
                    <div>
                      <dt>Provider</dt>
                      <dd>
                        {c.provider_name ? (
                          <>
                            <Link to="/providers">{c.provider_name}</Link>
                            <div className="muted" style={{ fontSize: '0.8rem' }}>
                              {c.provider_url}
                              {c.provider_node ? ` · ${c.provider_node}` : ''}
                            </div>
                          </>
                        ) : (
                          <span className="badge error">missing</span>
                        )}
                      </dd>
                    </div>
                    <div><dt>Base VMID</dt><dd>{c.cp_vmid ?? '—'}</dd></div>
                    <div><dt>Created</dt><dd className="muted">{c.created_at}</dd></div>
                  </dl>
                </section>
              </div>

              {latestJob && (
                <section className="overview-job">
                  <div className="section-head">
                    <h3 className="section-label">Latest job</h3>
                    <button type="button" className="secondary btn-icon" onClick={() => setTab('jobs')}>
                      View jobs
                    </button>
                  </div>
                  <div className="job-chip">
                    <span className="mono-inline">{latestJob.kind}</span>
                    <span className={`badge ${latestJob.status}`}>{latestJob.status}</span>
                    <span className="muted">{latestJob.updated_at}</span>
                  </div>
                </section>
              )}
            </div>
          )}

          {tab === 'nodes' && (
            <div className="tab-body">
              <div className="section-head">
                <div>
                  <h3 className="section-label">Nodes</h3>
                  <p className="muted">
                    {cps.length} control plane · {wks.length} worker
                    {nodes.length > 0 && (
                      <>
                        {' · '}
                        <span className="badge ready">{nodes.filter((n) => n.status === 'ready').length} ready</span>
                        {' '}
                        <span className="badge provisioning">
                          {nodes.filter((n) => n.status === 'provisioning' || n.status === 'pending').length} provisioning
                        </span>
                        {nodes.some((n) => n.status === 'error') && (
                          <>
                            {' '}
                            <span className="badge error">
                              {nodes.filter((n) => n.status === 'error').length} error
                            </span>
                          </>
                        )}
                      </>
                    )}
                  </p>
                </div>
                <div className="row-actions">
                  {selCount > 0 && (
                    <>
                      <button
                        type="button"
                        className="secondary btn-icon"
                        onClick={rebootSelected}
                        disabled={busy}
                      >
                        <Icon name="reboot" size={14} /> Reboot ({selCount})
                      </button>
                      <button
                        type="button"
                        className="danger btn-icon"
                        onClick={removeSelected}
                        disabled={busy}
                      >
                        <Icon name="trash" size={14} /> Remove ({selCount})
                      </button>
                    </>
                  )}
                  <button type="button" className="btn-icon" onClick={openAddModal}>
                    <Icon name="plus" size={16} /> Add node
                  </button>
                </div>
              </div>

              <NodesTable
                nodes={nodes}
                clusterId={id}
                dualStack={dualStack}
                showK8s
                showHw
                selectable
                selected={selectedNodes}
                onToggle={toggleNode}
                onToggleAll={toggleAll}
                onHardware={openHardware}
                onReboot={rebootNode}
              />
              {nodes.length === 0 && (
                <p className="muted empty-hint">
                  {c.status === 'pending' || c.status === 'provisioning'
                    ? 'Waiting for create job to seed nodes…'
                    : 'No nodes yet.'}
                </p>
              )}
              {nodes.some((n) => n.status === 'provisioning' || n.status === 'pending') && (
                <p className="muted empty-hint">
                  Live status updates as Proxmox creates VMs and nodes join — also watch the Jobs tab.
                </p>
              )}
            </div>
          )}

          {tab === 'config' && (
            <div className="tab-body tab-body-fill">
              <h3 className="section-label">Machine config</h3>
              <p className="muted">
                Apply YAML to all nodes via pertiskctl. Partial updates merge with each
                node&apos;s on-disk config (cluster / network preserved).{' '}
                <code className="mono-inline">machine.type</code> is set per node role
                so workers are not flipped to controlplane.
              </p>
              <textarea
                className="config-editor"
                value={configYaml}
                onChange={(e) => setConfigYaml(e.target.value)}
                spellCheck={false}
              />
              <div className="form-footer" style={{ marginBottom: 0 }}>
                <button type="button" className="btn-icon" onClick={applyConfig}>
                  <Icon name="check" size={16} /> Apply to all nodes
                </button>
              </div>
            </div>
          )}

          {tab === 'upgrade' && (
            <div className="tab-body upgrade-tab">
              <h3 className="section-label">Rolling upgrade</h3>
              <p className="muted">
                kubeadm-shaped rolling upgrade for near zero downtime: control planes
                one-by-one, then workers. Each node: drain → apply new version → wait Ready → uncordon.
                Requires HA (M≥3 + VIP) for API continuity; workers always drain for workload ZD.
              </p>
              <div className="upgrade-form">
                <div className="field">
                  <label>Target version</label>
                  <K8sVersionSelect
                    value={upgradeVer}
                    onChange={setUpgradeVer}
                    preferImage={false}
                  />
                </div>
                <button type="button" className="btn-icon" onClick={upgrade} disabled={upgradeRunning}>
                  <Icon name="play" size={16} /> {upgradeRunning ? 'Upgrade running…' : 'Start rolling upgrade'}
                </button>
              </div>
              <section className="upgrade-nodes">
                <h3 className="section-label">Node versions</h3>
                <p className="muted">
                  Cluster target: <span className="mono-inline">{c.k8s_version}</span>
                  {upgradeRunning && upgradeVer ? (
                    <> · upgrading toward <span className="mono-inline">{upgradeVer}</span></>
                  ) : null}
                </p>
                {nodes.length === 0 ? (
                  <p className="muted empty-hint">No nodes yet.</p>
                ) : (
                  <NodesTable
                    nodes={nodes}
                    clusterId={id}
                    dualStack={dualStack}
                    showK8s
                    showHw
                    targetVersion={upgradeRunning ? upgradeVer : null}
                  />
                )}
              </section>
            </div>
          )}

          {tab === 'jobs' && (
            <div className="tab-body jobs-tab tab-body-fill">
              <div className="jobs-layout">
                <div className="jobs-list">
                  <h3 className="section-label">History</h3>
                  {jobs.length === 0 && <p className="muted">No jobs yet.</p>}
                  {jobs.map((j) => (
                    <button
                      key={j.id}
                      type="button"
                      className={`job-item ${selectedJob === j.id ? 'active' : ''}`}
                      onClick={() => loadJobLog(j.id)}
                    >
                      <div className="job-item-top">
                        <span className="mono-inline">{j.kind}</span>
                        <span className={`badge ${j.status}`}>{j.status}</span>
                      </div>
                      <span className="muted">{j.updated_at}</span>
                      {j.error && <span className="error job-err">{j.error}</span>}
                    </button>
                  ))}
                </div>
                <div className="jobs-log">
                  <div className="section-head log-head">
                    <h3 className="section-label">Log</h3>
                    <button
                      type="button"
                      className={`secondary btn-icon ${followLog ? 'follow-on' : ''}`}
                      onClick={() => {
                        setFollowLog(true)
                        requestAnimationFrame(() => {
                          if (logRef.current) {
                            logRef.current.scrollTop = logRef.current.scrollHeight
                          }
                        })
                      }}
                      title="Follow latest output"
                    >
                      {followLog ? 'Following' : 'Follow'}
                    </button>
                  </div>
                  <pre
                    ref={logRef}
                    className="log-box mono"
                    onScroll={onLogScroll}
                  >
                    {log || '(select a job)'}
                  </pre>
                </div>
              </div>
            </div>
          )}
        </div>
      </div>

      <Modal
        open={addOpen}
        title="Add node"
        icon="plus"
        onClose={() => setAddOpen(false)}
        wide
      >
        <p className="modal-hint">
          Creates a Proxmox VM, waits for DHCP, and joins the cluster. Watch Jobs for live progress;
          the node shows as <span className="badge provisioning">provisioning</span> until ready.
        </p>
        <div className="field">
          <label>Role</label>
          <div className="role-pills">
            <button
              type="button"
              className={`role-pill ${addForm.role === 'worker' ? 'active' : ''}`}
              onClick={() => setAddRole('worker')}
            >
              <strong>Worker</strong>
              <span>Compute capacity</span>
            </button>
            <button
              type="button"
              className={`role-pill ${addForm.role === 'controlplane' ? 'active' : ''}`}
              onClick={() => setAddRole('controlplane')}
            >
              <strong>Control plane</strong>
              <span>Keep odd count for etcd</span>
            </button>
          </div>
        </div>
        <div className="field">
          <label>Count</label>
          <input
            type="number"
            min={1}
            max={16}
            value={addForm.count}
            onChange={(e) => setAddForm((f) => ({ ...f, count: e.target.value }))}
          />
        </div>
        <div className="field-row">
          <div className="field">
            <label>vCPU</label>
            <input
              type="number"
              min={1}
              value={addForm.cores}
              onChange={(e) => setAddForm((f) => ({ ...f, cores: e.target.value }))}
            />
          </div>
          <div className="field">
            <label>Memory (MB)</label>
            <input
              type="number"
              min={512}
              step={256}
              value={addForm.memory}
              onChange={(e) => setAddForm((f) => ({ ...f, memory: e.target.value }))}
            />
          </div>
          <div className="field">
            <label>Disk (GiB)</label>
            <input
              type="number"
              min={10}
              value={addForm.disk_gb}
              onChange={(e) => setAddForm((f) => ({ ...f, disk_gb: e.target.value }))}
            />
          </div>
        </div>
        <div className="modal-actions">
          <button type="button" className="secondary" onClick={() => setAddOpen(false)}>
            Cancel
          </button>
          <button type="button" onClick={submitAdd} disabled={busy}>
            {busy ? 'Queuing…' : 'Add node'}
          </button>
        </div>
      </Modal>

      <Modal
        open={hwOpen}
        title={hwNode ? `Hardware · ${hwNode.name}` : 'Hardware'}
        icon="cpu"
        onClose={() => {
          setHwOpen(false)
          setHwNode(null)
        }}
        wide
      >
        <p className="modal-hint">
          Resize this VM on Proxmox. Disk can only grow. Changing CPU, memory, or disk will stop and start the VM so sizes apply inside the guest (EPHEMERAL /var expands on boot after a disk grow).
        </p>
        <div className="field-row">
          <div className="field">
            <label>vCPU</label>
            <input
              type="number"
              min={1}
              value={hwForm.cores}
              onChange={(e) => setHwForm((f) => ({ ...f, cores: e.target.value }))}
            />
          </div>
          <div className="field">
            <label>Memory (MB)</label>
            <input
              type="number"
              min={512}
              step={256}
              value={hwForm.memory}
              onChange={(e) => setHwForm((f) => ({ ...f, memory: e.target.value }))}
            />
          </div>
          <div className="field">
            <label>Disk (GiB)</label>
            <input
              type="number"
              min={hwNode?.disk_gb ?? 10}
              value={hwForm.disk_gb}
              onChange={(e) => setHwForm((f) => ({ ...f, disk_gb: e.target.value }))}
            />
            {hwNode?.disk_gb != null && (
              <p className="muted" style={{ fontSize: '0.75rem', margin: '0.25rem 0 0' }}>
                Minimum {hwNode.disk_gb} GiB — disk can only grow.
              </p>
            )}
          </div>
        </div>
        {hwNode && (
          <p className="muted" style={{ fontSize: '0.8rem', marginTop: 0 }}>
            Current: {formatHw(hwNode)}
            {hwNode.vmid != null ? ` · VMID ${hwNode.vmid}` : ''}
          </p>
        )}
        <div className="modal-actions">
          <button
            type="button"
            className="secondary"
            onClick={() => {
              setHwOpen(false)
              setHwNode(null)
            }}
          >
            Cancel
          </button>
          <button type="button" onClick={submitHardware} disabled={busy}>
            {busy ? 'Queuing…' : 'Apply hardware'}
          </button>
        </div>
      </Modal>
    </div>
  )
}
