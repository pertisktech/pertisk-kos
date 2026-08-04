import { useCallback, useEffect, useRef, useState } from 'react'
import { Link, useNavigate, useParams, useSearchParams } from 'react-router-dom'
import { api, getToken } from '../api'
import { Icon } from '../components/Icons'
import { useConfirm } from '../components/Confirm'
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
      {(dualStack || v6) && v6 && <div className="mono-inline node-ip6">{v6}</div>}
    </div>
  )
}

function NodesTable({ nodes, dualStack, showK8s = true, targetVersion, onRemove }) {
  return (
    <table>
      <thead>
        <tr>
          <th>Name</th>
          <th>Role</th>
          <th>VMID</th>
          <th>{dualStack ? 'IPv4 / IPv6' : 'IP'}</th>
          {showK8s && <th>K8s</th>}
          <th>Status</th>
          {onRemove && <th></th>}
        </tr>
      </thead>
      <tbody>
        {nodes.map((n) => {
          const atTarget = targetVersion && n.k8s_version === targetVersion
          const upgrading = n.status === 'upgrading'
          return (
            <tr key={n.id} className={upgrading ? 'row-upgrading' : ''}>
              <td>{n.name}</td>
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
              <td><span className={`badge ${n.status}`}>{n.status}</span></td>
              {onRemove && (
                <td>
                  <button
                    type="button"
                    className="danger btn-icon"
                    onClick={() => onRemove(n.id, n.name)}
                    title="Remove node"
                  >
                    <Icon name="trash" size={14} />
                  </button>
                </td>
              )}
            </tr>
          )
        })}
      </tbody>
    </table>
  )
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
  const [configYaml, setConfigYaml] = useState('# machine config yaml\n')
  const [followLog, setFollowLog] = useState(true)
  const logRef = useRef(null)
  const selectedJobRef = useRef(null)

  const tab = TABS.some((t) => t.id === search.get('tab'))
    ? search.get('tab')
    : 'overview'

  function setTab(id) {
    setSearch(id === 'overview' ? {} : { tab: id }, { replace: true })
  }

  const load = useCallback(async () => {
    try {
      const d = await api(`/clusters/${id}`)
      setData(d)
      setUpgradeVer((prev) => prev || d?.cluster?.k8s_version || '')
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
    const t = setInterval(load, 4000)
    return () => clearInterval(t)
  }, [load])

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
      await api(`/clusters/${id}`, { method: 'DELETE' })
      nav('/clusters')
    } catch (err) {
      setError(err.message)
    }
  }

  async function addNode(role) {
    await api(`/clusters/${id}/nodes`, { method: 'POST', body: { role } })
    load()
  }

  async function removeNode(nid, name) {
    const ok = await confirm({
      title: 'Remove node',
      message: `Remove node “${name}” and destroy its VM?`,
      confirmLabel: 'Remove',
      tone: 'danger',
    })
    if (!ok) return
    await api(`/clusters/${id}/nodes/${nid}`, { method: 'DELETE' })
    load()
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
  const upgradeRunning = jobs.some((j) => j.kind === 'upgrade_cluster' && j.status === 'running')

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
                <section>
                  <h3 className="section-label">Resources</h3>
                  <div className="panel-grid tight">
                    <div className="mini-panel">
                      <Icon name="cpu" size={18} />
                      <div>
                        <strong>Control plane</strong>
                        <p className="muted">{c.cp_memory} MB · {c.cp_cores} vCPU · {c.cp_disk_gb} GiB</p>
                      </div>
                    </div>
                    <div className="mini-panel">
                      <Icon name="worker" size={18} />
                      <div>
                        <strong>Workers</strong>
                        <p className="muted">{c.worker_memory} MB · {c.worker_cores} vCPU · {c.worker_disk_gb} GiB</p>
                      </div>
                    </div>
                  </div>
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
                  <p className="muted">{cps.length} control plane · {wks.length} worker</p>
                </div>
                <div className="row-actions">
                  <button type="button" className="secondary btn-icon" onClick={() => addNode('worker')}>
                    <Icon name="plus" size={16} /> Worker
                  </button>
                  <button type="button" className="secondary btn-icon" onClick={() => addNode('controlplane')}>
                    <Icon name="plus" size={16} /> CP
                  </button>
                </div>
              </div>
              <NodesTable
                nodes={nodes}
                dualStack={dualStack}
                showK8s
                onRemove={removeNode}
              />
              {nodes.length === 0 && (
                <p className="muted empty-hint">Nodes appear after the create job finishes.</p>
              )}
            </div>
          )}

          {tab === 'config' && (
            <div className="tab-body tab-body-fill">
              <h3 className="section-label">Machine config</h3>
              <p className="muted">Apply YAML to all nodes via pertiskctl (queued job).</p>
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
                    dualStack={dualStack}
                    showK8s
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
    </div>
  )
}
