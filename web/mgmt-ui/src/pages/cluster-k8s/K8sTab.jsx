import { useCallback, useEffect, useMemo, useState } from 'react'
import { Icon } from '../../components/Icons'
import EmptyState from '../../components/EmptyState'
import { useConfirm } from '../../components/Confirm'
import { useShellDock } from '../../shell/ShellDockContext'
import {
  RESOURCE_GROUPS,
  deleteResource,
  listNamespaces,
  listResources,
  restartDeployment,
  scaleDeployment,
} from './api'
import ResourceTable from './ResourceTable'
import ResourceDrawer from './ResourceDrawer'
import DeployTab from './DeployTab'
import PodLogsPanel from './PodLogsPanel'
import { useMgmtRefresh } from '../../hooks/useMgmtEvents'

export default function K8sTab({ clusterId, ready }) {
  const confirm = useConfirm()
  const shellDock = useShellDock()
  const [view, setView] = useState('explore') // explore | deploy
  const [kind, setKind] = useState('deployments')
  const [namespace, setNamespace] = useState('all')
  const [namespaces, setNamespaces] = useState([])
  const [rows, setRows] = useState([])
  const [filter, setFilter] = useState('')
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')
  const [selected, setSelected] = useState(null)
  const [logsPod, setLogsPod] = useState(null)

  const clusterScoped = kind === 'nodes'

  const loadNs = useCallback(async () => {
    if (!ready || !clusterId) return
    try {
      const res = await listNamespaces(clusterId)
      setNamespaces(res.data || [])
    } catch {
      /* optional */
    }
  }, [clusterId, ready])

  const load = useCallback(async () => {
    if (!ready || !clusterId) return
    setLoading(true)
    setError('')
    try {
      const res = await listResources(clusterId, kind, clusterScoped ? undefined : namespace)
      setRows(res.data || [])
    } catch (e) {
      setError(e.message || 'failed to load resources')
      setRows([])
    } finally {
      setLoading(false)
    }
  }, [clusterId, kind, namespace, ready, clusterScoped])

  useEffect(() => {
    loadNs()
  }, [loadNs])

  useEffect(() => {
    load()
  }, [load])
  useMgmtRefresh(load, { clusterId })

  const filtered = useMemo(() => {
    const q = filter.trim().toLowerCase()
    if (!q) return rows
    return rows.filter(
      (r) =>
        String(r.name || '').toLowerCase().includes(q) ||
        String(r.namespace || '').toLowerCase().includes(q) ||
        String(r.status || '').toLowerCase().includes(q),
    )
  }, [rows, filter])

  async function onScale(row) {
    const raw = window.prompt(`Replicas for ${row.namespace}/${row.name}`, String(row.replicas ?? 1))
    if (raw == null) return
    const replicas = Number(raw)
    if (!Number.isFinite(replicas) || replicas < 0) return
    try {
      await scaleDeployment(clusterId, row.namespace, row.name, replicas)
      load()
    } catch (e) {
      setError(e.message)
    }
  }

  async function onRestart(row) {
    const ok = await confirm({
      title: 'Restart deployment',
      message: `Rollout restart ${row.namespace}/${row.name}?`,
      confirmLabel: 'Restart',
      tone: 'primary',
    })
    if (!ok) return
    try {
      await restartDeployment(clusterId, row.namespace, row.name)
      load()
    } catch (e) {
      setError(e.message)
    }
  }

  async function onDelete(row) {
    const ok = await confirm({
      title: 'Delete resource',
      message: `Delete ${kind} ${row.namespace ? `${row.namespace}/` : ''}${row.name}?`,
      confirmLabel: 'Delete',
      tone: 'danger',
    })
    if (!ok) return
    try {
      await deleteResource(clusterId, kind, row.namespace, row.name)
      if (selected?.name === row.name) setSelected(null)
      load()
    } catch (e) {
      setError(e.message)
    }
  }

  function onLogs(row) {
    setLogsPod(row)
    setSelected(null)
  }

  async function onExec(row) {
    const ok = await confirm({
      title: 'Pod exec',
      message: `Open an interactive shell in ${row.namespace}/${row.name}? Requires operator or admin.`,
      confirmLabel: 'Exec',
      tone: 'primary',
    })
    if (!ok) return
    shellDock.openPodExec?.(clusterId, row.namespace, row.name, {
      containers: row.containers,
      container: row.containers?.[0],
    })
  }

  if (!ready) {
    return (
      <div className="tab-body">
        <EmptyState
          icon="cpu"
          title="Kubernetes unavailable"
          message="Workloads and deploy tools are available when the cluster is ready and a kubeconfig has been stored."
        />
      </div>
    )
  }

  if (view === 'deploy') {
    return (
      <div className="k8s-explorer">
        <div className="k8s-explorer-top">
          <div className="k8s-kinds">
            <button type="button" className="tab-btn" onClick={() => setView('explore')}>
              Explorer
            </button>
            <button type="button" className="tab-btn active" onClick={() => setView('deploy')}>
              Deploy
            </button>
          </div>
        </div>
        <DeployTab clusterId={clusterId} ready={ready} />
      </div>
    )
  }

  return (
    <div className="k8s-explorer">
      <div className="k8s-explorer-top">
        <div>
          <h3 className="section-label">Kubernetes</h3>
          <p className="muted">Live resource explorer via kubectl on the management host.</p>
        </div>
        <div className="k8s-kinds">
          <button type="button" className="tab-btn active" onClick={() => setView('explore')}>
            Explorer
          </button>
          <button type="button" className="tab-btn" onClick={() => setView('deploy')}>
            Deploy
          </button>
          <button type="button" className="secondary btn-icon" onClick={load} disabled={loading}>
            <Icon name="refresh" size={14} /> Refresh
          </button>
        </div>
      </div>

      {error && <div className="error">{error}</div>}

      {logsPod && (
        <PodLogsPanel
          clusterId={clusterId}
          namespace={logsPod.namespace}
          name={logsPod.name}
          containers={logsPod.containers || []}
          onClose={() => setLogsPod(null)}
        />
      )}

      <div className="k8s-explorer-grid">
        <aside className="k8s-kind-rail" aria-label="Resource kinds">
          {RESOURCE_GROUPS.map((g) => (
            <div key={g.id} className="k8s-kind-group">
              <p className="nav-heading">{g.label}</p>
              <ul>
                {g.kinds.map((k) => (
                  <li key={k.id}>
                    <button
                      type="button"
                      className={kind === k.id ? 'active' : undefined}
                      onClick={() => {
                        setKind(k.id)
                        setSelected(null)
                      }}
                    >
                      {k.label}
                    </button>
                  </li>
                ))}
              </ul>
            </div>
          ))}
        </aside>

        <div className="k8s-explorer-main">
          <div className="k8s-toolbar">
            {!clusterScoped && (
              <label className="k8s-field">
                <span className="muted">Namespace</span>
                <select value={namespace} onChange={(e) => setNamespace(e.target.value)}>
                  <option value="all">All namespaces</option>
                  {namespaces.map((n) => (
                    <option key={n.name} value={n.name}>
                      {n.name}
                    </option>
                  ))}
                </select>
              </label>
            )}
            <label className="k8s-field k8s-field-grow">
              <span className="muted">Filter</span>
              <input
                type="search"
                value={filter}
                onChange={(e) => setFilter(e.target.value)}
                placeholder="Filter by name…"
              />
            </label>
          </div>

          <ResourceTable
            kind={kind}
            rows={filtered}
            onSelect={setSelected}
            onScale={onScale}
            onRestart={onRestart}
            onDelete={onDelete}
            onLogs={onLogs}
            onExec={onExec}
          />
        </div>
      </div>

      <ResourceDrawer
        open={!!selected}
        onClose={() => setSelected(null)}
        clusterId={clusterId}
        kind={kind}
        row={selected}
        onLogs={onLogs}
        onExec={onExec}
      />
    </div>
  )
}
