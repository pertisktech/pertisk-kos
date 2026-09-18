import { Icon } from './Icons'
import { ClusterStatusBadges } from './ClusterStatusBadges'
import { ClusterMetaBadges, formatProviderKind } from './ClusterMetaBadges'
import ResourceGauge, { GAUGE_BASE } from './ResourceGauge'

export default function ClusterCard({ summary, onOpen, compact = false }) {
  const version = formatK8sVersion(summary.k8s_version)
  const cps = Number(summary.controlplanes) || 0
  const wks = Number(summary.workers) || 0
  const nodes = Number(summary.node_count) || cps + wks
  const statusClass = summary.status || 'unknown'
  const avail = summary.availability || 'unknown'
  const provider = summary.provider_name || formatProviderKind(summary.provider_kind)
  const cardClass = [
    'cluster-card',
    compact ? 'cluster-card-compact' : '',
    `status-${statusClass}`,
    statusClass === 'ready' ? `avail-${avail}` : '',
    summary._placeholder ? 'cluster-resource-skeleton' : '',
  ]
    .filter(Boolean)
    .join(' ')

  return (
    <article
      className={cardClass}
      role="link"
      tabIndex={0}
      onClick={onOpen}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault()
          onOpen()
        }
      }}
    >
      <div className="cluster-card-head">
        <div className="cluster-card-identity">
          <span className="cluster-card-icon" aria-hidden>
            <Icon name="radio" size={compact ? 18 : 20} />
          </span>
          <div className="cluster-card-title">
            <p className="cluster-card-name">{summary.cluster_name}</p>
            <p className="cluster-card-meta">
              {version ? <span className="mono-inline">{version}</span> : null}
              {version && provider ? <span aria-hidden>·</span> : null}
              {provider ? <span>{provider}</span> : null}
            </p>
          </div>
        </div>
        <ClusterStatusBadges status={summary.status} availability={summary.availability} />
      </div>

      <div className="cluster-card-stats">
        <div className="cluster-card-stat">
          <p className="cluster-card-stat-label">
            <Icon name="machines" size={12} /> Nodes
          </p>
          <p className="cluster-card-stat-value">{nodes}</p>
        </div>
        <div className="cluster-card-stat">
          <p className="cluster-card-stat-label">Control</p>
          <p className="cluster-card-stat-value">{cps}</p>
        </div>
        <div className="cluster-card-stat">
          <p className="cluster-card-stat-label">Workers</p>
          <p className="cluster-card-stat-value">{wks}</p>
        </div>
      </div>

      <div className="cluster-card-body">
        <div className="cluster-card-tags">
          {(summary.arch || summary.provider_kind) && (
            <ClusterMetaBadges arch={summary.arch} providerKind={summary.provider_kind} />
          )}
          {summary.vip ? (
            <span className="tag tag-outline">
              <Icon name="network" size={12} /> VIP {summary.vip}
            </span>
          ) : null}
        </div>
        <div className="cluster-card-meters">
          <ResourceGauge label="CPU" icon="cpu" metric={summary.cpu} color={GAUGE_BASE.cpu} layout="row" />
          <ResourceGauge label="Memory" icon="memory" metric={summary.memory} color={GAUGE_BASE.memory} layout="row" />
          <ResourceGauge label="Disk" icon="disk" metric={summary.disk} color={GAUGE_BASE.disk} layout="row" />
        </div>
        {summary.error && summary.status === 'ready' && (
          <p className="muted cluster-resource-soft-err" title={summary.error}>
            <Icon name="alert" size={12} />
            {summary.error}
          </p>
        )}
      </div>
    </article>
  )
}

export function formatK8sVersion(v) {
  if (!v) return null
  const s = String(v).trim()
  if (!s) return null
  return s.startsWith('v') ? s : `v${s}`
}

export function placeholderSummary(c) {
  const nodes = (c.controlplanes || 0) + (c.workers || 0)
  const empty = { used: null, total: null, percent: null, unit: '', display_used: null, display_total: null, error: null }
  return {
    cluster_id: c.id,
    cluster_name: c.name,
    status: c.status || 'unknown',
    availability: c.availability || 'unknown',
    k8s_version: c.k8s_version || '',
    node_count: nodes,
    controlplanes: c.controlplanes || 0,
    workers: c.workers || 0,
    provider_kind: c.provider_kind,
    provider_name: c.provider_name,
    arch: c.arch,
    vip: c.vip,
    cpu: { ...empty, unit: 'cores' },
    memory: { ...empty, unit: 'GiB' },
    disk: { ...empty, unit: 'GiB' },
    error: null,
    _placeholder: true,
  }
}
