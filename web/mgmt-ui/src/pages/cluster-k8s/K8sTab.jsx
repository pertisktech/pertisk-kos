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
} from './api'
import ResourceTable from './ResourceTable'
import ResourceDetailPanel from './ResourceDetailPanel'
import DeployTab from './DeployTab'
import { useMgmtRefresh } from '../../hooks/useMgmtEvents'

const KIND_TITLES = Object.fromEntries(
  RESOURCE_GROUPS.flatMap((g) => g.kinds.map((k) => [k.id, k.label])),
)

function rowKey(r) {
  return r ? `${r.namespace || ''}/${r.name}` : ''
}

export default function K8sTab({ clusterId, ready }) {
  const confirm = useConfirm()
  const shellDock = useShellDock()
  const [view, setView] = useState('explore') // explore | deploy
  const [kind, setKind] = useState('deployments')
  const [openGroups, setOpenGroups] = useState(() =>
    Object.fromEntries(RESOURCE_GROUPS.map((g) => [g.id, true])),
  )
  const [namespace, setNamespace] = useState('all')
  const [namespaces, setNamespaces] = useState([])
  const [rows, setRows] = useState([])
  const [filter, setFilter] = useState('')
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')
  const [selected, setSelected] = useState(null)

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
    if (!ready || !clusterId || view !== 'explore') return
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
  }, [clusterId, kind, namespace, ready, clusterScoped, view])

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

  function selectKind(nextKind) {
    setView('explore')
    setKind(nextKind)
    setSelected(null)
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
      if (rowKey(selected) === rowKey(row)) setSelected(null)
      load()
    } catch (e) {
      setError(e.message)
    }
  }

  function onLogs(row) {
    setSelected(null)
    shellDock.openPodLogs?.(clusterId, row.namespace, row.name, {
      containers: row.containers,
      container: row.containers?.[0],
    })
  }

  async function onExec(row) {
    const ok = await confirm({
      title: 'Pod exec',
      message: `Open an interactive shell in ${row.namespace}/${row.name}? Requires operator or admin.`,
      confirmLabel: 'Exec',
      tone: 'primary',
    })
    if (!ok) return
    setSelected(null)
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

  return (
    <div className={`k8s-shell${selected && view === 'explore' ? ' has-detail' : ''}`}>
      <aside className="k8s-nav" aria-label="Kubernetes resources">
        <div className="k8s-nav-brand">
          <Icon name="cpu" size={16} />
          <span>Cluster</span>
        </div>

        {RESOURCE_GROUPS.map((g) => {
          const open = openGroups[g.id] !== false
          return (
            <div key={g.id} className="k8s-nav-group">
              <button
                type="button"
                className="k8s-nav-group-toggle"
                onClick={() => setOpenGroups((prev) => ({ ...prev, [g.id]: !open }))}
                aria-expanded={open}
              >
                <Icon name={open ? 'chevron-down' : 'chevrons-right'} size={12} />
                <span>{g.label}</span>
              </button>
              {open && (
                <ul>
                  {g.kinds.map((k) => (
                    <li key={k.id}>
                      <button
                        type="button"
                        className={view === 'explore' && kind === k.id ? 'active' : undefined}
                        onClick={() => selectKind(k.id)}
                      >
                        {k.label}
                      </button>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          )
        })}

        <div className="k8s-nav-group">
          <p className="k8s-nav-label">Apps</p>
          <ul>
            <li>
              <button
                type="button"
                className={view === 'deploy' ? 'active' : undefined}
                onClick={() => {
                  setView('deploy')
                  setSelected(null)
                }}
              >
                Deploy / Helm
              </button>
            </li>
          </ul>
        </div>
      </aside>

      <div className="k8s-main">
        {view === 'deploy' ? (
          <DeployTab clusterId={clusterId} ready={ready} />
        ) : (
          <>
            <div className="k8s-main-head">
              <div>
                <h3 className="k8s-page-title">{KIND_TITLES[kind] || kind}</h3>
                <p className="muted">Live via kubectl on the management host</p>
              </div>
              <div className="k8s-main-toolbar">
                {!clusterScoped && (
                  <select
                    className="k8s-select"
                    value={namespace}
                    onChange={(e) => setNamespace(e.target.value)}
                    aria-label="Namespace"
                  >
                    <option value="all">All Namespaces</option>
                    {namespaces.map((n) => (
                      <option key={n.name} value={n.name}>
                        {n.name}
                      </option>
                    ))}
                  </select>
                )}
                <input
                  className="k8s-filter"
                  type="search"
                  value={filter}
                  onChange={(e) => setFilter(e.target.value)}
                  placeholder="Filter by name…"
                  aria-label="Filter resources"
                />
                <button
                  type="button"
                  className="k8s-icon-btn"
                  onClick={load}
                  disabled={loading}
                  title="Refresh"
                  aria-label="Refresh"
                >
                  <Icon name="refresh" size={14} />
                </button>
              </div>
            </div>

            {error && <div className="error">{error}</div>}

            <ResourceTable
              kind={kind}
              rows={filtered}
              selectedKey={rowKey(selected)}
              onSelect={setSelected}
              onLogs={onLogs}
              onExec={onExec}
            />
          </>
        )}
      </div>

      {selected && view === 'explore' && (
        <>
          <button
            type="button"
            className="k8s-detail-scrim"
            aria-label="Close details"
            onClick={() => setSelected(null)}
          />
          <ResourceDetailPanel
            clusterId={clusterId}
            kind={kind}
            row={selected}
            onClose={() => setSelected(null)}
            onDeleted={onDelete}
            onLogs={onLogs}
            onExec={onExec}
            onMutated={load}
          />
        </>
      )}
    </div>
  )
}
