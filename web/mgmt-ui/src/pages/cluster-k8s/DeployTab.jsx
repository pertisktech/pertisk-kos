import { lazy, Suspense, useCallback, useEffect, useState } from 'react'
import { Icon } from '../../components/Icons'
import EmptyState from '../../components/EmptyState'
import { useConfirm } from '../../components/Confirm'
import {
  applyYaml,
  getHelmRelease,
  helmInstall,
  helmUninstall,
  listHelmReleases,
} from './api'
import { useMgmtRefresh } from '../../hooks/useMgmtEvents'

const YamlEditor = lazy(() => import('../../components/YamlEditor'))

const EMPTY_HELM = {
  name: '',
  namespace: 'default',
  chart: '',
  repo: '',
  version: '',
  values_yaml: '',
  create_namespace: true,
}

export default function DeployTab({ clusterId, ready }) {
  const confirm = useConfirm()
  const [mode, setMode] = useState('releases') // releases | helm | yaml
  const [releases, setReleases] = useState([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')
  const [form, setForm] = useState(EMPTY_HELM)
  const [yamlDoc, setYamlDoc] = useState('')
  const [busy, setBusy] = useState(false)
  const [detail, setDetail] = useState(null)

  const load = useCallback(async () => {
    if (!ready || !clusterId) return
    setLoading(true)
    setError('')
    try {
      const res = await listHelmReleases(clusterId)
      setReleases(res.data || [])
    } catch (e) {
      setError(e.message || 'failed to list Helm releases')
      setReleases([])
    } finally {
      setLoading(false)
    }
  }, [clusterId, ready])

  useEffect(() => {
    load()
  }, [load])
  useMgmtRefresh(load, { clusterId })

  async function onInstall(e) {
    e.preventDefault()
    setBusy(true)
    setError('')
    try {
      await helmInstall(clusterId, {
        name: form.name.trim(),
        namespace: form.namespace.trim() || 'default',
        chart: form.chart.trim(),
        repo: form.repo.trim() || undefined,
        version: form.version.trim() || undefined,
        values_yaml: form.values_yaml || undefined,
        create_namespace: form.create_namespace,
      })
      setMode('releases')
      setForm(EMPTY_HELM)
      await load()
    } catch (err) {
      setError(err.message || 'helm install failed')
    } finally {
      setBusy(false)
    }
  }

  async function onApplyYaml(e) {
    e.preventDefault()
    setBusy(true)
    setError('')
    try {
      await applyYaml(clusterId, yamlDoc)
      setYamlDoc('')
      setMode('releases')
      await load()
    } catch (err) {
      setError(err.message || 'apply failed')
    } finally {
      setBusy(false)
    }
  }

  async function onUninstall(r) {
    const ns = r.namespace || r.Namespace
    const name = r.name || r.Name
    const ok = await confirm({
      title: 'Uninstall Helm release',
      message: `Uninstall ${ns}/${name}?`,
      confirmLabel: 'Uninstall',
      tone: 'danger',
    })
    if (!ok) return
    setBusy(true)
    try {
      await helmUninstall(clusterId, ns, name)
      setDetail(null)
      await load()
    } catch (err) {
      setError(err.message)
    } finally {
      setBusy(false)
    }
  }

  async function openDetail(r) {
    const ns = r.namespace || r.Namespace
    const name = r.name || r.Name
    setError('')
    try {
      const res = await getHelmRelease(clusterId, ns, name)
      setDetail(res)
      setMode('releases')
    } catch (err) {
      setError(err.message)
    }
  }

  if (!ready) {
    return (
      <div className="tab-body">
        <EmptyState
          icon="apps"
          title="Deploy unavailable"
          message="Helm and YAML deploy need a ready cluster with a stored kubeconfig."
        />
      </div>
    )
  }

  return (
    <div className="tab-body k8s-deploy-tab">
      <div className="section-head">
        <div>
          <h3 className="section-label">Deploy</h3>
          <p className="muted">Helm releases and raw YAML apply (Devtron-lite). Curated add-ons stay on the Apps tab.</p>
        </div>
        <div className="k8s-kinds">
          <button type="button" className={mode === 'releases' ? 'tab-btn active' : 'tab-btn'} onClick={() => setMode('releases')}>
            Releases
          </button>
          <button type="button" className={mode === 'helm' ? 'tab-btn active' : 'tab-btn'} onClick={() => setMode('helm')}>
            Helm install
          </button>
          <button type="button" className={mode === 'yaml' ? 'tab-btn active' : 'tab-btn'} onClick={() => setMode('yaml')}>
            YAML apply
          </button>
          <button type="button" className="secondary btn-icon" onClick={load} disabled={loading}>
            <Icon name="refresh" size={14} /> Refresh
          </button>
        </div>
      </div>

      {error && <div className="error">{error}</div>}

      {mode === 'releases' && (
        <>
          <div className="table-wrap">
            <table>
              <thead>
                <tr>
                  <th>Name</th>
                  <th>Namespace</th>
                  <th>Chart</th>
                  <th>Revision</th>
                  <th>Status</th>
                  <th>Updated</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {releases.length === 0 && (
                  <tr>
                    <td colSpan={7} className="muted">
                      {loading ? 'Loading…' : 'No Helm releases'}
                    </td>
                  </tr>
                )}
                {releases.map((r) => {
                  const name = r.name || r.Name
                  const ns = r.namespace || r.Namespace
                  return (
                    <tr key={`${ns}/${name}`}>
                      <td className="mono-inline">{name}</td>
                      <td>{ns}</td>
                      <td className="muted">{r.chart || r.Chart || '—'}</td>
                      <td className="mono-inline">{r.revision ?? r.Revision ?? '—'}</td>
                      <td>
                        <span className="badge ready">{r.status || r.Status || '—'}</span>
                      </td>
                      <td className="muted">{r.updated || r.Updated || '—'}</td>
                      <td className="row-actions">
                        <button type="button" className="secondary btn-icon" onClick={() => openDetail(r)}>
                          Values
                        </button>
                        <button type="button" className="danger btn-icon" onClick={() => onUninstall(r)} disabled={busy}>
                          <Icon name="trash" size={14} />
                        </button>
                      </td>
                    </tr>
                  )
                })}
              </tbody>
            </table>
          </div>
          {detail && (
            <div className="k8s-helm-detail">
              <div className="section-head">
                <h4 className="section-label">
                  {detail.namespace}/{detail.name}
                </h4>
                <button type="button" className="secondary btn-icon" onClick={() => setDetail(null)}>
                  Close
                </button>
              </div>
              <pre className="pod-logs-body" style={{ maxHeight: 320 }}>
                {detail.values || '(no values)'}
              </pre>
            </div>
          )}
        </>
      )}

      {mode === 'helm' && (
        <form className="k8s-deploy-form" onSubmit={onInstall}>
          <div className="form-grid-2">
            <label>
              Release name
              <input
                required
                value={form.name}
                onChange={(e) => setForm({ ...form, name: e.target.value })}
              />
            </label>
            <label>
              Namespace
              <input
                required
                value={form.namespace}
                onChange={(e) => setForm({ ...form, namespace: e.target.value })}
              />
            </label>
            <label>
              Chart
              <input
                required
                placeholder="bitnami/nginx or oci://… or chart name with repo"
                value={form.chart}
                onChange={(e) => setForm({ ...form, chart: e.target.value })}
              />
            </label>
            <label>
              Chart repo URL (optional)
              <input
                placeholder="https://charts.example.com"
                value={form.repo}
                onChange={(e) => setForm({ ...form, repo: e.target.value })}
              />
            </label>
            <label>
              Version (optional)
              <input
                value={form.version}
                onChange={(e) => setForm({ ...form, version: e.target.value })}
              />
            </label>
            <label className="checkbox-row">
              <input
                type="checkbox"
                checked={form.create_namespace}
                onChange={(e) => setForm({ ...form, create_namespace: e.target.checked })}
              />
              Create namespace
            </label>
          </div>
          <label>
            Values YAML (optional)
            <Suspense fallback={<p className="muted">Loading editor…</p>}>
              <div className="yaml-editor-frame" style={{ height: 220 }}>
                <YamlEditor
                  value={form.values_yaml}
                  onChange={(v) => setForm({ ...form, values_yaml: v })}
                  path="helm-values"
                  schema="kubeconfig"
                />
              </div>
            </Suspense>
          </label>
          <button type="submit" className="btn" disabled={busy}>
            {busy ? 'Installing…' : 'helm upgrade --install'}
          </button>
        </form>
      )}

      {mode === 'yaml' && (
        <form className="k8s-deploy-form" onSubmit={onApplyYaml}>
          <p className="muted">Multi-document YAML applied with kubectl apply -f - on the management host.</p>
          <Suspense fallback={<p className="muted">Loading editor…</p>}>
            <div className="yaml-editor-frame" style={{ height: 360 }}>
              <YamlEditor value={yamlDoc} onChange={setYamlDoc} path="k8s-apply" schema="kubeconfig" />
            </div>
          </Suspense>
          <button type="submit" className="btn" disabled={busy || !yamlDoc.trim()}>
            {busy ? 'Applying…' : 'Apply YAML'}
          </button>
        </form>
      )}
    </div>
  )
}
