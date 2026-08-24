import { useCallback, useEffect, useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router-dom'
import { api } from '../api'
import { Icon } from '../components/Icons'
import ResourceGauge, { GAUGE_BASE } from '../components/ResourceGauge'
import { ProviderStatusBadge } from '../components/ProviderStatusBadge'
import { formatProviderKind, normalizeProviderKind } from '../components/ClusterMetaBadges'
import { useMgmtRefresh } from '../hooks/useMgmtEvents'

const POLL_MS = 15000

export default function ProviderDashboard() {
  const { id } = useParams()
  const nav = useNavigate()
  const [summary, setSummary] = useState(null)
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(true)

  const load = useCallback(() => {
    if (!id) return
    api(`/providers/${id}/dashboard`)
      .then((row) => {
        setSummary(row)
        setError('')
      })
      .catch((e) => {
        setError(e.message || 'failed to load provider')
      })
      .finally(() => setLoading(false))
  }, [id])

  useEffect(() => {
    load()
    const t = setInterval(load, POLL_MS)
    return () => clearInterval(t)
  }, [load])
  useMgmtRefresh(load)

  if (loading && !summary) {
    return (
      <div>
        <div className="page-head">
          <h1><Icon name="providers" size={22} /> Provider</h1>
        </div>
        <p className="muted">Loading hypervisor stats…</p>
      </div>
    )
  }

  if (!summary) {
    return (
      <div>
        <div className="page-head">
          <h1><Icon name="providers" size={22} /> Provider</h1>
          <Link className="btn secondary btn-icon" to="/providers">
            <Icon name="back" size={16} /> Back
          </Link>
        </div>
        {error && <div className="error">{error}</div>}
      </div>
    )
  }

  const kind = normalizeProviderKind(summary.kind)

  return (
    <div>
      <div className="page-head">
        <div>
          <h1>
            <Icon name="providers" size={22} /> {summary.provider_name}
          </h1>
          <div className="detail-title-meta" style={{ marginTop: 8, display: 'flex', gap: 8, flexWrap: 'wrap', alignItems: 'center' }}>
            <ProviderStatusBadge availability={summary.availability} showUnknown />
            <span className={`badge kind kind-${kind}`}>{formatProviderKind(summary.kind)}</span>
            {summary.node && <span className="cluster-resource-chip">{summary.node}</span>}
            {summary.storage && <span className="cluster-resource-chip mono-inline">{summary.storage}</span>}
          </div>
        </div>
        <div className="row-actions">
          <Link className="btn secondary btn-icon" to="/providers">
            <Icon name="back" size={16} /> Providers
          </Link>
          <button type="button" className="secondary btn-icon" onClick={load}>
            <Icon name="refresh" size={14} /> Refresh
          </button>
        </div>
      </div>

      {error && <div className="error">{error}</div>}

      <p className="muted">
        Live hypervisor capacity · CPU, memory, and disk as used / available / total.
        {summary.url ? ` · ${summary.url}` : ''}
      </p>

      <div className="card" style={{ padding: '1.25rem' }}>
        <div className="resource-gauge-row provider-dash-gauges">
          <ResourceGauge label="CPU" icon="cpu" metric={summary.cpu} color={GAUGE_BASE.cpu} />
          <ResourceGauge label="Memory" icon="memory" metric={summary.memory} color={GAUGE_BASE.memory} />
          <ResourceGauge label="Disk" icon="disk" metric={summary.disk} color={GAUGE_BASE.disk} />
        </div>
        {summary.error && (
          <p className="muted cluster-resource-soft-err" title={summary.error}>
            <Icon name="alert" size={12} />
            {summary.error}
          </p>
        )}
      </div>

      <p className="muted chart-footnote">
        Disk is the provider storage ({summary.storage || 'configured datastore / container'}).
        CPU and memory are the selected host
        {kind === 'nutanix' ? ' (AHV cluster sum when the node name is the Prism cluster)' : ''}.
        {' '}
        <button type="button" className="linkish" onClick={() => nav('/providers')}>
          Manage providers
        </button>
      </p>
    </div>
  )
}
