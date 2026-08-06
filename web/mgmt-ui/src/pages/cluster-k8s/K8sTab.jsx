import { useCallback, useEffect, useState } from 'react'
import { Icon } from '../../components/Icons'
import { useConfirm } from '../../components/Confirm'
import {
  WORKLOAD_KINDS,
  deleteWorkload,
  listNamespaces,
  listWorkloads,
  restartDeployment,
  scaleDeployment,
} from './api'
import WorkloadTable from './WorkloadTable'

const POLL_MS = 5000

export default function K8sTab({ clusterId, ready }) {
  const confirm = useConfirm()
  const [kind, setKind] = useState('deployments')
  const [namespace, setNamespace] = useState('all')
  const [namespaces, setNamespaces] = useState([])
  const [rows, setRows] = useState([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')

  const loadNs = useCallback(async () => {
    if (!ready || !clusterId) return
    try {
      const res = await listNamespaces(clusterId)
      setNamespaces(res.data || [])
    } catch {
      /* namespaces optional */
    }
  }, [clusterId, ready])

  const load = useCallback(async () => {
    if (!ready || !clusterId) return
    setLoading(true)
    setError('')
    try {
      const res = await listWorkloads(clusterId, kind, namespace)
      setRows(res.data || [])
    } catch (e) {
      setError(e.message || 'failed to load workloads')
      setRows([])
    } finally {
      setLoading(false)
    }
  }, [clusterId, kind, namespace, ready])

  useEffect(() => {
    loadNs()
  }, [loadNs])

  useEffect(() => {
    load()
    if (!ready) return undefined
    const t = setInterval(load, POLL_MS)
    return () => clearInterval(t)
  }, [load, ready])

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
      message: `Delete ${kind} ${row.namespace}/${row.name}?`,
      confirmLabel: 'Delete',
      tone: 'danger',
    })
    if (!ok) return
    try {
      await deleteWorkload(clusterId, kind, row.namespace, row.name)
      load()
    } catch (e) {
      setError(e.message)
    }
  }

  if (!ready) {
    return (
      <div className="tab-body">
        <p className="muted">
          K8s workloads are available when the cluster status is <span className="badge ready">ready</span>
          {' '}and a kubeconfig has been stored.
        </p>
      </div>
    )
  }

  return (
    <div className="tab-body tab-body-fill k8s-tab">
      <div className="section-head">
        <div>
          <h3 className="section-label">Workloads</h3>
          <p className="muted">Live view via kubectl on the management host (polls every {POLL_MS / 1000}s).</p>
        </div>
        <button type="button" className="secondary btn-icon" onClick={load} disabled={loading}>
          <Icon name="play" size={14} /> Refresh
        </button>
      </div>

      {error && <div className="error">{error}</div>}

      <div className="k8s-toolbar">
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
        <div className="k8s-kinds">
          {WORKLOAD_KINDS.map((k) => (
            <button
              key={k.id}
              type="button"
              className={kind === k.id ? 'tab-btn active' : 'tab-btn'}
              onClick={() => setKind(k.id)}
            >
              {k.label}
            </button>
          ))}
        </div>
      </div>

      <WorkloadTable
        kind={kind}
        rows={rows}
        onScale={onScale}
        onRestart={onRestart}
        onDelete={onDelete}
      />
    </div>
  )
}
