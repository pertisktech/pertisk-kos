import { lazy, Suspense, useCallback, useEffect, useRef, useState } from 'react'
import { Link, useNavigate, useParams, useSearchParams } from 'react-router-dom'
import { api, getToken } from '../api'
import { defaultMachineConfigYaml } from '../utils/machineConfig'
import { Icon } from '../components/Icons'
import { ClusterStatusBadges } from '../components/ClusterStatusBadges'
import { ClusterMetaBadges, formatArch, formatProviderKind, normalizeProviderKind } from '../components/ClusterMetaBadges'
import { NodeStatusBadges } from '../components/NodeStatusBadges'
import { useConfirm } from '../components/Confirm'
import Checkbox from '../components/Checkbox'
import ColorLogViewer from '../components/ColorLogViewer'
import Modal from '../components/Modal'
import K8sVersionSelect from '../components/K8sVersionSelect'
import OsBundlePicker, { osBundleReady } from '../components/OsBundlePicker'
import { useMgmtRefresh } from '../hooks/useMgmtEvents'
import K8sTab from './cluster-k8s/K8sTab'
import ShellTab from './cluster-k8s/ShellTab'

const YamlEditor = lazy(() => import('../components/YamlEditor'))

const TABS = [
  { id: 'overview', label: 'Overview', icon: 'dashboard' },
  { id: 'nodes', label: 'Nodes', icon: 'worker' },
  { id: 'k8s', label: 'K8s', icon: 'cpu' },
  { id: 'shell', label: 'Shell', icon: 'play' },
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

const VERSION_SOURCE = {
  nodes: 'nodes',
  cluster: 'cluster',
  image: 'image pin',
}

function VersionsTable({ components }) {
  if (!components?.length) return null
  return (
    <section className="overview-versions">
      <div className="section-head">
        <h3 className="section-label">Components</h3>
        <Link to="/os-packages" className="muted">OS packages</Link>
      </div>
      <table className="versions-table">
        <thead>
          <tr>
            <th>Component</th>
            <th>Version</th>
            <th>Target</th>
            <th>Source</th>
          </tr>
        </thead>
        <tbody>
          {components.map((row) => {
            const target = row.desired && row.desired !== row.version ? row.desired : null
            return (
              <tr key={row.id}>
                <td>{row.name}</td>
                <td>
                  {row.mixed ? (
                    <div className="versions-mixed">
                      <span className="badge">mixed</span>
                      {(row.nodes || []).map((n) => (
                        <div key={n.name} className="muted mono-inline">
                          {n.name} · {n.version || '—'}
                        </div>
                      ))}
                    </div>
                  ) : (
                    <span className="mono-inline">{row.version || '—'}</span>
                  )}
                </td>
                <td>
                  {target ? (
                    row.id === 'os' ? (
                      <Link className="mono-inline" to="/os-packages">{target}</Link>
                    ) : row.id === 'kubernetes' ? (
                      <Link className="mono-inline" to={`?tab=upgrade`}>{target}</Link>
                    ) : (
                      <span className="mono-inline">{target}</span>
                    )
                  ) : (
                    <span className="muted">—</span>
                  )}
                </td>
                <td className="muted">{VERSION_SOURCE[row.source] || row.source}</td>
              </tr>
            )
          })}
        </tbody>
      </table>
    </section>
  )
}

function NodesTable({
  nodes,
  clusterId,
  dualStack,
  showK8s = true,
  showOs = false,
  showHw = false,
  targetVersion,
  targetOsVersion,
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
          <th>Source</th>
          <th>{dualStack ? 'IPv4 / IPv6' : 'IP'}</th>
          {showK8s && <th>K8s</th>}
          {showOs && <th>OS</th>}
          {showHw && <th>Hardware</th>}
          <th>Status</th>
          {(onHardware || onReboot) && <th className="col-actions" />}
        </tr>
      </thead>
      <tbody>
        {nodes.map((n) => {
          const atTarget = targetVersion && n.k8s_version === targetVersion
          const atOsTarget = targetOsVersion && n.os_version === targetOsVersion
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
              <td className="muted">
                {n.source === 'adopted' || n.source === 'baremetal'
                  ? n.source
                  : n.vmid != null
                    ? `${n.source || 'proxmox'} #${n.vmid}`
                    : n.source || '—'}
              </td>
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
              {showOs && (
                <td>
                  <span className={`mono-inline ${atOsTarget ? 'ver-match' : ''}`}>
                    {n.os_version || '—'}
                  </span>
                  {targetOsVersion && !atOsTarget && (
                    <span className="muted ver-arrow"> → {targetOsVersion}</span>
                  )}
                </td>
              )}
              {showHw && <td className="hw-cell">{formatHw(n)}</td>}
              <td>
                <NodeStatusBadges status={n.status} availability={n.availability} />
              </td>
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

function errorBannerKey(clusterId, jobId, message) {
  return `pertisk:err-banner:${clusterId}:${jobId || message.slice(0, 120)}`
}

function isErrorBannerDismissed(key) {
  try {
    return sessionStorage.getItem(key) === '1'
  } catch {
    return false
  }
}

function dismissErrorBanner(key) {
  try {
    sessionStorage.setItem(key, '1')
  } catch {
    /* ignore */
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
  const [osBundle, setOsBundle] = useState(null)
  const [osTargetVer, setOsTargetVer] = useState('')
  const [osPackages, setOsPackages] = useState([])
  const [osPackageId, setOsPackageId] = useState('')
  const [configYaml, setConfigYaml] = useState(() => defaultMachineConfigYaml(''))
  const configTouched = useRef(false)
  const [templates, setTemplates] = useState([])
  const [templateId, setTemplateId] = useState('')
  const [followLog, setFollowLog] = useState(true)
  const [selectedNodes, setSelectedNodes] = useState(() => new Set())
  const [addOpen, setAddOpen] = useState(false)
  const [addMode, setAddMode] = useState('create') // create | adopt | join
  const [addForm, setAddForm] = useState({
    role: 'worker',
    count: 1,
    memory: 8192,
    cores: 4,
    disk_gb: 75,
    ip: '',
    name: '',
    source: 'adopted',
  })
  const [joinTokens, setJoinTokens] = useState([])
  const [joinDetail, setJoinDetail] = useState(null)
  const [joinBusy, setJoinBusy] = useState(false)
  const [hwOpen, setHwOpen] = useState(false)
  const [hwNode, setHwNode] = useState(null)
  const [hwForm, setHwForm] = useState({ memory: 4096, cores: 2, disk_gb: 50 })
  const [kubeconfigOpen, setKubeconfigOpen] = useState(false)
  const [kubeconfigFilenameShown, setKubeconfigFilenameShown] = useState('')
  const [kubeconfigText, setKubeconfigText] = useState('')
  const [kubeCopied, setKubeCopied] = useState(false)
  const [busy, setBusy] = useState(false)
  const [errorDismissedKey, setErrorDismissedKey] = useState('')
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

  useMgmtRefresh(load, { clusterId: id })

  useEffect(() => {
    let cancelled = false
    api('/settings')
      .then((s) => {
        if (cancelled || configTouched.current) return
        const url = String(s?.public_url || '').trim()
        setConfigYaml(defaultMachineConfigYaml(url))
      })
      .catch(() => {
        /* keep theme/border default without mgmt_url */
      })
    return () => {
      cancelled = true
    }
  }, [])

  useEffect(() => {
    if (tab !== 'config') return undefined
    let cancelled = false
    api('/templates')
      .then((rows) => {
        if (!cancelled) setTemplates(Array.isArray(rows) ? rows : [])
      })
      .catch(() => {
        if (!cancelled) setTemplates([])
      })
    return () => {
      cancelled = true
    }
  }, [tab])

  useEffect(() => {
    if (tab !== 'upgrade') return undefined
    let cancelled = false
    api('/os-packages')
      .then((rows) => {
        if (!cancelled) setOsPackages(Array.isArray(rows) ? rows : [])
      })
      .catch(() => {
        if (!cancelled) setOsPackages([])
      })
    return () => {
      cancelled = true
    }
  }, [tab])

  useEffect(() => {
    load()
    const status = data?.cluster?.status
    const busy =
      status === 'provisioning' ||
      status === 'pending' ||
      status === 'upgrading' ||
      status === 'deleting'
    // While busy, poll logs (SSE covers status; logs still need a slow tick).
    const t = setInterval(load, busy ? 3000 : 20000)
    return () => clearInterval(t)
  }, [load, data?.cluster?.status])

  async function loadJobLog(jobId, { follow = true } = {}) {
    selectedJobRef.current = jobId
    setSelectedJob(jobId)
    const text = await api(`/jobs/${jobId}/log`).catch(() => '')
    setLog(text)
    if (follow) setFollowLog(true)
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
    setAddMode('create')
    setAddForm({ role: 'worker', count: 1, ip: '', name: '', source: 'adopted', ...d })
    setJoinDetail(null)
    setAddOpen(true)
    loadJoinTokens()
  }

  function setAddRole(role) {
    const d = defaultsForRole(data?.cluster, role)
    setAddForm((f) => ({ ...f, role, ...d }))
  }

  async function loadJoinTokens() {
    try {
      const rows = await api(`/clusters/${id}/join-tokens`)
      setJoinTokens(Array.isArray(rows) ? rows : [])
    } catch {
      setJoinTokens([])
    }
  }

  async function submitAdd() {
    setBusy(true)
    setError('')
    try {
      if (addMode === 'adopt') {
        const body = {
          role: addForm.role,
          ip: String(addForm.ip || '').trim(),
          source: addForm.source || 'adopted',
        }
        const nm = String(addForm.name || '').trim()
        if (nm) body.name = nm
        const res = await api(`/clusters/${id}/nodes/adopt`, { method: 'POST', body })
        setAddOpen(false)
        if (res?.job_id) selectJob(res.job_id)
        setTab('jobs')
        load()
        return
      }
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

  async function createJoinToken() {
    setJoinBusy(true)
    setError('')
    try {
      const detail = await api(`/clusters/${id}/join-tokens`, {
        method: 'POST',
        body: { role: addForm.role, label: addForm.name || '' },
      })
      setJoinDetail(detail)
      await loadJoinTokens()
    } catch (err) {
      setError(err.message)
    } finally {
      setJoinBusy(false)
    }
  }

  async function showJoinToken(tid) {
    setJoinBusy(true)
    try {
      const detail = await api(`/clusters/${id}/join-tokens/${tid}`)
      setJoinDetail(detail)
    } catch (err) {
      setError(err.message)
    } finally {
      setJoinBusy(false)
    }
  }

  async function revokeJoinToken(tid) {
    if (!window.confirm('Revoke this join-token snapshot? (Does not delete the kube bootstrap Secret.)')) {
      return
    }
    setJoinBusy(true)
    try {
      await api(`/clusters/${id}/join-tokens/${tid}`, { method: 'DELETE' })
      if (joinDetail?.id === tid) setJoinDetail(null)
      await loadJoinTokens()
    } catch (err) {
      setError(err.message)
    } finally {
      setJoinBusy(false)
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

  async function upgradeOs() {
    const fromCatalog = !!osPackageId
    if (!fromCatalog && !osBundleReady(osBundle)) return
    const ok = await confirm({
      title: 'OS A/B upgrade',
      message:
        'Upgrade the node OS (kernel + pertiskd) on all nodes?\n\nOrder: workers first, then control planes one-by-one.\nEach node: stage signed bundle → drain → reboot into the inactive slot → mark-boot-good → uncordon.\nKubernetes version is not changed. STATE and etcd stay on disk.',
      confirmLabel: 'Start OS upgrade',
      tone: 'primary',
    })
    if (!ok) return
    setBusy(true)
    setError('')
    try {
      let res
      if (fromCatalog) {
        res = await api(`/clusters/${id}/os-upgrade/package`, {
          method: 'POST',
          body: { package_id: osPackageId, reboot: true },
        })
      } else {
        const fd = new FormData()
        fd.append('reboot', 'true')
        if (osBundle.zip) {
          fd.append('bundle', osBundle.zip)
        } else {
          fd.append('kernel', osBundle.kernel)
          fd.append('initramfs', osBundle.initramfs)
          fd.append('manifest.json', osBundle.manifest)
          fd.append('manifest.sig', osBundle.sig)
        }
        res = await api(`/clusters/${id}/os-upgrade`, { method: 'POST', body: fd })
      }
      if (res?.version) setOsTargetVer(res.version)
      if (res?.job_id) selectJob(res.job_id)
      setTab('jobs')
      load()
    } catch (err) {
      setError(err.message)
    } finally {
      setBusy(false)
    }
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

  function kubeconfigFilename() {
    const clusterName = data?.cluster?.name || 'cluster'
    const safeName = String(clusterName)
      .trim()
      .replace(/[^a-zA-Z0-9._-]+/g, '-')
      .replace(/^[.-]+|[.-]+$/g, '') || 'kubeconfig'
    return safeName.endsWith('.yaml') || safeName.endsWith('.yml')
      ? safeName
      : `${safeName}.yaml`
  }

  function configBundleFilename() {
    const clusterName = data?.cluster?.name || 'cluster'
    let safeName = String(clusterName)
      .trim()
      .replace(/[^a-zA-Z0-9._-]+/g, '-')
      .replace(/^[.-]+|[.-]+$/g, '') || 'cluster'
    if (safeName.endsWith('.yaml') || safeName.endsWith('.yml')) {
      safeName = safeName.replace(/\.ya?ml$/i, '').replace(/-+$/g, '') || 'cluster'
    }
    return `${safeName}-config.zip`
  }

  async function fetchKubeconfig() {
    setError('')
    const res = await fetch(`/api/clusters/${id}/kubeconfig`, {
      headers: { Authorization: `Bearer ${getToken()}` },
    })
    if (!res.ok) {
      const body = await res.json().catch(() => ({ error: res.statusText }))
      throw new Error(body.error || res.statusText)
    }
    return res.text()
  }

  function triggerDownload(text, filename) {
    const blob = new Blob([text], { type: 'application/x-yaml;charset=utf-8' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = filename
    a.rel = 'noopener'
    document.body.appendChild(a)
    a.click()
    a.remove()
    URL.revokeObjectURL(url)
  }

  function triggerBlobDownload(blob, filename) {
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = filename
    a.rel = 'noopener'
    document.body.appendChild(a)
    a.click()
    a.remove()
    URL.revokeObjectURL(url)
  }

  async function downloadKc() {
    const filename = kubeconfigFilename()
    try {
      const text = await fetchKubeconfig()
      triggerDownload(text, filename)
      return { text, filename }
    } catch (err) {
      setError(err.message)
      return null
    }
  }

  async function downloadConfigBundle() {
    setError('')
    const filename = configBundleFilename()
    try {
      const res = await fetch(`/api/clusters/${id}/config-bundle`, {
        headers: { Authorization: `Bearer ${getToken()}` },
      })
      if (!res.ok) {
        const body = await res.json().catch(() => ({ error: res.statusText }))
        throw new Error(body.error || res.statusText)
      }
      const blob = await res.blob()
      triggerBlobDownload(blob, filename)
    } catch (err) {
      setError(err.message)
    }
  }

  async function showKubeconfig() {
    setError('')
    setKubeCopied(false)
    try {
      const text = await fetchKubeconfig()
      const filename = kubeconfigFilename()
      setKubeconfigText(text)
      setKubeconfigFilenameShown(filename)
      setKubeconfigOpen(true)
    } catch (err) {
      setError(err.message)
    }
  }

  async function copyKubeconfig() {
    setError('')
    setKubeCopied(false)
    try {
      let text = kubeconfigText
      if (!text || !kubeconfigOpen) {
        text = await fetchKubeconfig()
        setKubeconfigText(text)
        setKubeconfigFilenameShown(kubeconfigFilename())
      }
      await navigator.clipboard.writeText(text)
      setKubeCopied(true)
      setTimeout(() => setKubeCopied(false), 2000)
    } catch (err) {
      setError(err.message || 'Failed to copy kubeconfig')
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
  const versions = data.versions || []
  const netLabel = c.network_mode || (c.vip6 && c.vip ? 'dual-stack' : c.vip6 ? 'ipv6' : 'ipv4')
  const dualStack = netLabel === 'dual-stack' || netLabel === 'ipv6'
  const cps = nodes.filter((n) => n.role === 'controlplane')
  const wks = nodes.filter((n) => n.role !== 'controlplane')
  const latestJob = jobs[0]
  const selectedJobRow = jobs.find((j) => j.id === selectedJob)
  const createJob = jobs.find((j) => j.kind === 'create_cluster')
  const createRunning = createJob?.status === 'running' || createJob?.status === 'queued'
  const createFailed = createJob?.status === 'failed'
  const nodesWithoutIp = nodes.filter((n) => !n.ip?.trim())
  const hollowReady =
    c.status === 'ready' &&
    nodes.length > 0 &&
    nodesWithoutIp.length === nodes.length
  const upgradeRunning = jobs.some((j) => j.kind === 'upgrade_cluster' && j.status === 'running')
  const osUpgradeRunning = jobs.some((j) => j.kind === 'upgrade_os' && j.status === 'running')
  const clusterArch = formatArch(c.arch)
  const osPkgsForArch = osPackages.filter((p) => formatArch(p.arch) === clusterArch)
  // Banner follows the newest job only. A later success (or in-progress job)
  // must hide older failures — including sticky clusters.error from past runs.
  // Each failure is one-time: dismiss (or close) hides it for this browser tab.
  const latestFailedJob =
    latestJob?.status === 'failed' && latestJob?.error ? latestJob : null
  const rawDisplayError = (() => {
    if (!latestJob) {
      return c.status === 'error' ? c.error || '' : ''
    }
    if (
      latestJob.status === 'succeeded' ||
      latestJob.status === 'running' ||
      latestJob.status === 'queued'
    ) {
      return ''
    }
    if (latestJob.status === 'failed') {
      return latestJob.error || ''
    }
    return c.status === 'error' ? c.error || '' : ''
  })()
  const bannerKey = rawDisplayError
    ? errorBannerKey(id, latestFailedJob?.id, rawDisplayError)
    : ''
  const displayError =
    rawDisplayError &&
    bannerKey &&
    bannerKey !== errorDismissedKey &&
    !isErrorBannerDismissed(bannerKey)
      ? rawDisplayError
      : ''

  function dismissBanner() {
    if (!bannerKey) return
    dismissErrorBanner(bannerKey)
    setErrorDismissedKey(bannerKey)
  }

  const selCount = selectedNodes.size

  return (
    <div className="detail">
      <div className="detail-top">
      <div className="page-head">
        <div className="detail-title">
          <h1>
            <Icon name="clusters" size={22} /> {c.name}
          </h1>
          <div className="detail-title-meta">
            <ClusterStatusBadges status={c.status} availability={c.availability} />
            <ClusterMetaBadges arch={c.arch} providerKind={c.provider_kind} />
          </div>
        </div>
        <div className="row-actions">
          <Link className="btn secondary btn-icon" to="/clusters">
            <Icon name="back" size={16} /> Back
          </Link>
          <button type="button" className="secondary btn-icon" onClick={downloadKc} title="Download kubeconfig YAML">
            <Icon name="download" size={16} /> Download kubeconfig
          </button>
          <button
            type="button"
            className="secondary btn-icon"
            onClick={downloadConfigBundle}
            title="Download cluster-out ZIP (kubeconfig + machine/join YAMLs)"
          >
            <Icon name="download" size={16} /> Download config
          </button>
          <button type="button" className="danger btn-icon" onClick={del}>
            <Icon name="trash" size={16} /> Delete
          </button>
        </div>
      </div>

      {error && <div className="error">{error}</div>}
      {displayError && (
        <div className="banner danger">
          <Icon name="alert" size={18} />
          <div className="banner-error-body">
            <pre className="banner-error-text">{displayError}</pre>
            {latestFailedJob && (
              <p className="banner-error-meta muted">
                Job <span className="mono-inline">{latestFailedJob.kind}</span>
                {latestFailedJob.finished_at ? ` · ${latestFailedJob.finished_at}` : ''}
                {' · '}
                <button
                  type="button"
                  className="linkish"
                  onClick={() => {
                    setTab('jobs')
                    loadJobLog(latestFailedJob.id)
                  }}
                >
                  View full job log
                </button>
              </p>
            )}
          </div>
          <button
            type="button"
            className="banner-dismiss"
            aria-label="Dismiss error"
            title="Dismiss"
            onClick={dismissBanner}
          >
            <Icon name="x" size={16} />
          </button>
        </div>
      )}
      {createFailed && !displayError && (() => {
        const createKey = errorBannerKey(id, createJob?.id, createJob?.error || 'create_failed')
        if (createKey === errorDismissedKey || isErrorBannerDismissed(createKey)) return null
        return (
          <div className="banner danger">
            <Icon name="alert" size={18} />
            <div className="banner-error-body">
              <span>
                Create cluster failed{createJob?.error ? `: ${createJob.error}` : '.'}
                {' '}
                <button type="button" className="linkish" onClick={() => setTab('jobs')}>View job log</button>
              </span>
            </div>
            <button
              type="button"
              className="banner-dismiss"
              aria-label="Dismiss error"
              title="Dismiss"
              onClick={() => {
                dismissErrorBanner(createKey)
                setErrorDismissedKey(createKey)
              }}
            >
              <Icon name="x" size={16} />
            </button>
          </div>
        )
      })()}
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
          <div className="value sm cluster-provider-stat">
            {c.provider_name ? (
              <>
                <span className={`badge kind kind-${normalizeProviderKind(c.provider_kind)}`}>
                  {formatProviderKind(c.provider_kind)}
                </span>
                <span>{c.provider_name}</span>
              </>
            ) : (
              <span className="badge error">missing</span>
            )}
          </div>
          {c.provider_node && (
            <div className="muted" style={{ fontSize: '0.75rem', marginTop: 4 }}>
              {c.provider_node}
            </div>
          )}
        </div>
        <div className="stat">
          <div className="label">Arch</div>
          <div className="value sm">
            <span className={`badge arch arch-${c.arch === 'arm64' ? 'arm64' : 'amd64'}`}>
              {c.arch === 'arm64' ? 'arm64' : 'amd64'}
            </span>
          </div>
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
                    <div>
                      <dt>Status</dt>
                      <dd>
                        <ClusterStatusBadges status={c.status} availability={c.availability} />
                      </dd>
                    </div>
                    <div><dt>Endpoint</dt><dd className="mono-inline">{c.endpoint || '—'}</dd></div>
                    <div>
                      <dt>Provider</dt>
                      <dd>
                        {c.provider_name ? (
                          <>
                            <span className={`badge kind kind-${normalizeProviderKind(c.provider_kind)}`} style={{ marginRight: 6 }}>
                              {formatProviderKind(c.provider_kind)}
                            </span>
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
                    <div>
                      <dt>Arch</dt>
                      <dd>
                        <span className={`badge arch arch-${c.arch === 'arm64' ? 'arm64' : 'amd64'}`}>
                          {c.arch === 'arm64' ? 'arm64' : 'amd64'}
                        </span>
                      </dd>
                    </div>
                    <div><dt>Base VMID</dt><dd>{c.cp_vmid ?? '—'}</dd></div>
                    <div><dt>Created</dt><dd className="muted">{c.created_at}</dd></div>
                  </dl>
                </section>
                <section>
                  <h3 className="section-label">Network</h3>
                  <dl className="kv">
                    <div><dt>Mode</dt><dd>{netLabel}</dd></div>
                    <div><dt>CNI</dt><dd>{c.cni}</dd></div>
                    <div>
                      <dt>Pod CIDRs</dt>
                      <dd className="mono-inline">
                        {c.pod_subnet || '10.244.0.0/16'}
                        {c.pod_subnet_ipv6 ? <><br />{c.pod_subnet_ipv6}</> : null}
                      </dd>
                    </div>
                    <div>
                      <dt>Service CIDRs</dt>
                      <dd className="mono-inline">
                        {c.service_subnet || '10.96.0.0/12'}
                        {c.service_subnet_ipv6 ? <><br />{c.service_subnet_ipv6}</> : null}
                      </dd>
                    </div>
                    {(c.vip || c.vip6) && (
                      <div>
                        <dt>VIP</dt>
                        <dd className="mono-inline">
                          {c.vip || '—'}
                          {c.vip6 ? <><br />{c.vip6}</> : null}
                        </dd>
                      </div>
                    )}
                  </dl>
                </section>
              </div>

              <VersionsTable components={versions} />

              <section className="overview-job">
                <div className="section-head">
                  <h3 className="section-label">Kubeconfig</h3>
                  <div className="row-actions">
                    <button
                      type="button"
                      className="btn-icon"
                      onClick={copyKubeconfig}
                      disabled={c.status !== 'ready'}
                      title="Copy kubeconfig to clipboard"
                    >
                      <Icon name="check" size={16} /> {kubeCopied ? 'Copied' : 'Copy'}
                    </button>
                    <button
                      type="button"
                      className="secondary btn-icon"
                      onClick={showKubeconfig}
                      disabled={c.status !== 'ready'}
                      title="View kubeconfig content"
                    >
                      View
                    </button>
                    <button
                      type="button"
                      className="secondary btn-icon"
                      onClick={downloadKc}
                      disabled={c.status !== 'ready'}
                      title="Download kubeconfig file"
                    >
                      <Icon name="download" size={16} /> Download
                    </button>
                  </div>
                </div>
                <p className="muted" style={{ margin: 0 }}>
                  Copy or download this cluster’s kubeconfig for kubectl or kube-web (
                  <code className="mono-inline">KUBECONFIG</code>
                  ). Use <strong>Download config</strong> in the header for a ZIP of the
                  full cluster-out directory (kubeconfig + machine/join YAMLs).
                </p>
              </section>

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
                        <span className="badge online">
                          {nodes.filter((n) => n.availability === 'online').length} online
                        </span>
                        {nodes.some((n) => n.availability === 'offline') && (
                          <>
                            {' '}
                            <span className="badge offline">
                              {nodes.filter((n) => n.availability === 'offline').length} offline
                            </span>
                          </>
                        )}
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

          {tab === 'k8s' && (
            <K8sTab clusterId={id} ready={c.status === 'ready' && !hollowReady} />
          )}

          {tab === 'shell' && (
            <ShellTab
              clusterId={id}
              clusterName={c.name}
              ready={c.status === 'ready' && !hollowReady}
            />
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
              <div
                className="form-row"
                style={{ display: 'flex', gap: '0.75rem', flexWrap: 'wrap', marginBottom: '0.75rem' }}
              >
                <label className="field" style={{ flex: '1 1 16rem' }}>
                  Load template
                  <select
                    value={templateId}
                    onChange={(e) => {
                      const idSel = e.target.value
                      setTemplateId(idSel)
                      const t = templates.find((x) => x.id === idSel)
                      if (t?.yaml) {
                        configTouched.current = true
                        setConfigYaml(t.yaml)
                      }
                    }}
                  >
                    <option value="">— choose template —</option>
                    {templates.map((t) => (
                      <option key={t.id} value={t.id}>
                        {t.name} ({t.role})
                      </option>
                    ))}
                  </select>
                </label>
              </div>
              <Suspense fallback={<div className="yaml-editor yaml-editor--fill muted">Loading editor…</div>}>
                <YamlEditor
                  className="yaml-editor--fill"
                  schema="machine"
                  path={`cluster-${id}`}
                  value={configYaml}
                  onChange={(next) => {
                    configTouched.current = true
                    setConfigYaml(next)
                  }}
                />
              </Suspense>
              <div className="form-footer" style={{ marginBottom: 0 }}>
                <button type="button" className="btn-icon" onClick={applyConfig}>
                  <Icon name="check" size={16} /> Apply to all nodes
                </button>
              </div>
            </div>
          )}

          {tab === 'upgrade' && (
            <div className="tab-body upgrade-tab">
              <section className="upgrade-section">
                <h3 className="section-label">Kubernetes rolling upgrade</h3>
                <p className="muted">
                  kubeadm-shaped rolling upgrade for near zero downtime: control planes
                  one-by-one, then workers. Each node: drain → apply new version → wait Ready → uncordon.
                  Requires HA (M≥3 + VIP) for API continuity; workers always drain for workload ZD.
                  Does not change the node OS.
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
                  <button type="button" className="btn-icon" onClick={upgrade} disabled={upgradeRunning || osUpgradeRunning}>
                    <Icon name="play" size={16} /> {upgradeRunning ? 'Upgrade running…' : 'Start rolling upgrade'}
                  </button>
                </div>
              </section>

              <section className="upgrade-section">
                <h3 className="section-label">OS A/B upgrade</h3>
                <p className="muted">
                  Signed bundle only — Kubernetes is not changed. Workers first, then control planes.
                  Pick a catalog version or upload a new zip (
                  <Link to="/os-packages">OS packages</Link>
                  ). Trust key <span className="mono-inline">os-trust.pk</span> is installed on STATE
                  if missing. Recreating VMs from a new qcow2 is a reinstall, not this path.
                </p>
                <div className="os-upgrade-form">
                  <div className="field" style={{ width: '100%' }}>
                    <label>Catalog version</label>
                    <select
                      value={osPackageId}
                      onChange={(e) => {
                        setOsPackageId(e.target.value)
                        if (e.target.value) setOsBundle(null)
                      }}
                      disabled={osUpgradeRunning || busy}
                    >
                      <option value="">— upload files below —</option>
                      {osPkgsForArch.map((p) => (
                        <option key={p.id} value={p.id}>
                          {p.version} ({formatArch(p.arch)})
                        </option>
                      ))}
                    </select>
                    {osPkgsForArch.length === 0 && (
                      <span className="muted" style={{ display: 'block', marginTop: '0.35rem' }}>
                        No {clusterArch} packages yet.{' '}
                        <Link to="/os-packages">Upload on OS packages</Link>
                      </span>
                    )}
                  </div>
                  {!osPackageId && (
                    <OsBundlePicker
                      value={osBundle}
                      onChange={setOsBundle}
                      disabled={osUpgradeRunning || busy}
                    />
                  )}
                  <button
                    type="button"
                    className="btn-icon"
                    onClick={upgradeOs}
                    disabled={
                      (!osPackageId && !osBundleReady(osBundle)) ||
                      osUpgradeRunning ||
                      upgradeRunning ||
                      busy
                    }
                  >
                    <Icon name="play" size={16} /> {osUpgradeRunning ? 'OS upgrade running…' : 'Start OS upgrade'}
                  </button>
                </div>
              </section>

              <section className="upgrade-nodes">
                <h3 className="section-label">Node versions</h3>
                <p className="muted">
                  Cluster K8s: <span className="mono-inline">{c.k8s_version}</span>
                  {upgradeRunning && upgradeVer ? (
                    <> · upgrading toward <span className="mono-inline">{upgradeVer}</span></>
                  ) : null}
                  {osUpgradeRunning && osTargetVer ? (
                    <> · OS → <span className="mono-inline">{osTargetVer}</span></>
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
                    showOs
                    showHw
                    targetVersion={upgradeRunning ? upgradeVer : null}
                    targetOsVersion={osUpgradeRunning ? osTargetVer : null}
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
                    <h3 className="section-label">
                      <Icon name="providers" size={16} /> Log
                    </h3>
                    <div className="row-actions node-log-actions">
                      <button
                        type="button"
                        className="secondary btn-icon"
                        onClick={() => selectedJob && loadJobLog(selectedJob, { follow: false })}
                        disabled={!selectedJob}
                        title="Refresh now"
                      >
                        Refresh
                      </button>
                      <button
                        type="button"
                        className={`secondary btn-icon ${followLog ? 'follow-on' : ''}`}
                        onClick={() => setFollowLog(true)}
                        title="Follow latest output"
                      >
                        {followLog ? 'Following' : 'Follow'}
                      </button>
                    </div>
                  </div>
                  {selectedJobRow && (
                    <p className="muted chart-footnote">
                      {selectedJobRow.kind} · {selectedJobRow.status}
                    </p>
                  )}
                  <ColorLogViewer
                    text={log}
                    empty="(select a job)"
                    follow={followLog}
                    onFollowChange={setFollowLog}
                    className="jobs-log-box"
                    aria-label="Job log"
                  />
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
        <div className="role-pills" style={{ marginBottom: '1rem' }}>
          <button
            type="button"
            className={`role-pill ${addMode === 'create' ? 'active' : ''}`}
            onClick={() => setAddMode('create')}
          >
            <strong>Create VM</strong>
            <span>Proxmox guest + join</span>
          </button>
          <button
            type="button"
            className={`role-pill ${addMode === 'adopt' ? 'active' : ''}`}
            onClick={() => setAddMode('adopt')}
          >
            <strong>Adopt</strong>
            <span>Existing node by IP</span>
          </button>
          <button
            type="button"
            className={`role-pill ${addMode === 'join' ? 'active' : ''}`}
            onClick={() => {
              setAddMode('join')
              loadJoinTokens()
            }}
          >
            <strong>Join instructions</strong>
            <span>Bootstrap token copy</span>
          </button>
        </div>

        {addMode !== 'join' && (
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
        )}

        {addMode === 'create' && (
          <>
            <p className="modal-hint">
              Creates a Proxmox VM, waits for DHCP, and joins the cluster. Watch Jobs for live progress;
              the node shows as <span className="badge provisioning">provisioning</span> until ready.
            </p>
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
          </>
        )}

        {addMode === 'adopt' && (
          <>
            <p className="modal-hint">
              Joins a machine that already runs Pertisk KOS (Machine API on :50000). No VM is created —
              remove only drains/deletes the Kubernetes node.
            </p>
            <div className="field">
              <label>Node IP</label>
              <input
                value={addForm.ip}
                onChange={(e) => setAddForm((f) => ({ ...f, ip: e.target.value }))}
                placeholder="10.1.1.50"
                autoComplete="off"
              />
            </div>
            <div className="field">
              <label>Hostname (optional)</label>
              <input
                value={addForm.name}
                onChange={(e) => setAddForm((f) => ({ ...f, name: e.target.value }))}
                placeholder="defaults to cluster-wk-N / cluster-cp-N"
                autoComplete="off"
              />
            </div>
            <div className="field">
              <label>Source tag</label>
              <select
                value={addForm.source}
                onChange={(e) => setAddForm((f) => ({ ...f, source: e.target.value }))}
              >
                <option value="adopted">adopted</option>
                <option value="baremetal">baremetal</option>
              </select>
            </div>
          </>
        )}

        {addMode === 'join' && (
          <>
            <p className="modal-hint">
              Snapshots the cluster bootstrap token from <code className="mono-inline">worker.yaml</code> for
              copy/paste. Prefer <strong>Adopt</strong> when mgmt can reach the node. Revoke only hides the
              snapshot — it does not rotate the kube Secret.
            </p>
            <div className="field">
              <label>Role for instructions</label>
              <div className="role-pills">
                <button
                  type="button"
                  className={`role-pill ${addForm.role === 'worker' ? 'active' : ''}`}
                  onClick={() => setAddRole('worker')}
                >
                  <strong>Worker</strong>
                  <span>apply join-config</span>
                </button>
                <button
                  type="button"
                  className={`role-pill ${addForm.role === 'controlplane' ? 'active' : ''}`}
                  onClick={() => setAddRole('controlplane')}
                >
                  <strong>Control plane</strong>
                  <span>get-join-config + etcd</span>
                </button>
              </div>
            </div>
            <div className="field">
              <label>Label (optional)</label>
              <input
                value={addForm.name}
                onChange={(e) => setAddForm((f) => ({ ...f, name: e.target.value }))}
                placeholder="e.g. rack-a bare metal"
              />
            </div>
            <div className="row-actions" style={{ marginBottom: '0.75rem' }}>
              <button type="button" className="btn-icon" onClick={createJoinToken} disabled={joinBusy}>
                <Icon name="plus" size={14} /> {joinBusy ? 'Working…' : 'Create snapshot'}
              </button>
              <button type="button" className="secondary btn-icon" onClick={loadJoinTokens} disabled={joinBusy}>
                Refresh
              </button>
            </div>
            {joinTokens.length > 0 && (
              <table style={{ marginBottom: '0.75rem' }}>
                <thead>
                  <tr>
                    <th>Created</th>
                    <th>Role</th>
                    <th>Label</th>
                    <th>Status</th>
                    <th />
                  </tr>
                </thead>
                <tbody>
                  {joinTokens.map((t) => (
                    <tr key={t.id}>
                      <td className="muted">{t.created_at}</td>
                      <td>{t.role}</td>
                      <td>{t.label || '—'}</td>
                      <td>
                        <span className={`badge ${t.revoked_at ? 'error' : 'ready'}`}>
                          {t.revoked_at ? 'revoked' : 'active'}
                        </span>
                      </td>
                      <td className="col-actions">
                        <div className="row-actions-cell">
                          <button
                            type="button"
                            className="secondary btn-icon"
                            onClick={() => showJoinToken(t.id)}
                            disabled={joinBusy}
                          >
                            Show
                          </button>
                          {!t.revoked_at && (
                            <button
                              type="button"
                              className="danger btn-icon"
                              onClick={() => revokeJoinToken(t.id)}
                              disabled={joinBusy}
                            >
                              Revoke
                            </button>
                          )}
                        </div>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
            {joinDetail && (
              <pre className="log-box mono" style={{ maxHeight: '16rem' }}>
                {joinDetail.instructions
                  || `endpoint: ${joinDetail.endpoint || ''}\ntoken: ${joinDetail.token || ''}`}
              </pre>
            )}
          </>
        )}

        <div className="modal-actions">
          <button type="button" className="secondary" onClick={() => setAddOpen(false)}>
            Cancel
          </button>
          {addMode !== 'join' && (
            <button
              type="button"
              onClick={submitAdd}
              disabled={busy || (addMode === 'adopt' && !String(addForm.ip || '').trim())}
            >
              {busy ? 'Queuing…' : addMode === 'adopt' ? 'Adopt node' : 'Add node'}
            </button>
          )}
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

      <Modal
        open={kubeconfigOpen}
        title="Kubeconfig"
        icon="download"
        cardClassName="modal-yaml"
        onClose={() => {
          setKubeconfigOpen(false)
          setKubeCopied(false)
        }}
      >
        <p className="muted" style={{ marginTop: 0 }}>
          <code className="mono-inline">{kubeconfigFilenameShown || 'cluster.yaml'}</code>
        </p>
        <Suspense fallback={<div className="yaml-editor yaml-editor--modal muted">Loading editor…</div>}>
          <YamlEditor
            className="yaml-editor--modal"
            schema="kubeconfig"
            path={`cluster-${id}`}
            value={kubeconfigText}
            readOnly
          />
        </Suspense>
        <div className="modal-actions">
          <button
            type="button"
            className="secondary"
            onClick={() => {
              setKubeconfigOpen(false)
              setKubeCopied(false)
            }}
          >
            Close
          </button>
          <button
            type="button"
            className="secondary btn-icon"
            onClick={() => {
              if (kubeconfigText) triggerDownload(kubeconfigText, kubeconfigFilenameShown || 'kubeconfig.yaml')
            }}
          >
            <Icon name="download" size={16} /> Download
          </button>
          <button type="button" className="btn-icon" onClick={copyKubeconfig}>
            <Icon name="check" size={16} /> {kubeCopied ? 'Copied' : 'Copy'}
          </button>
        </div>
      </Modal>
    </div>
  )
}
