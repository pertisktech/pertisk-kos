import { Icon } from './Icons'
import { ClusterStatusBadges } from './ClusterStatusBadges'
import { ClusterMetaBadges } from './ClusterMetaBadges'
import ResourceGauge, { GAUGE_BASE } from './ResourceGauge'

export default function ClusterCard({ summary, onOpen }) {
  const version = formatK8sVersion(summary.k8s_version)
  const nodes = summary.node_count
  const statusClass = summary.status || 'unknown'
  const avail = summary.availability || 'unknown'
  const cardClass = [
    'cluster-card',
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
            <Icon name="clusters" size={18} />
          </span>
          <div className="cluster-card-title">
            <p className="cluster-card-name">{summary.cluster_name}</p>
            <p className="cluster-card-meta">
              {version ? <span className="mono-inline">{version}</span> : null}
              {version ? <span aria-hidden>·</span> : null}
              <span>
                {nodes} node{nodes === 1 ? '' : 's'}
              </span>
            </p>
          </div>
        </div>
        <ClusterStatusBadges status={summary.status} availability={summary.availability} />
      </div>
      <div className="cluster-card-tags">
        {(summary.arch || summary.provider_kind) && (
          <ClusterMetaBadges arch={summary.arch} providerKind={summary.provider_kind} />
        )}
        {summary.vip ? <span className="tag tag-outline">VIP {summary.vip}</span> : null}
      </div>
      <div className="cluster-card-meters">
        <ResourceGauge label="CPU" icon="cpu" metric={summary.cpu} color={GAUGE_BASE.cpu} size="lg" />
        <ResourceGauge label="Memory" icon="memory" metric={summary.memory} color={GAUGE_BASE.memory} size="lg" />
        <ResourceGauge label="Disk" icon="disk" metric={summary.disk} color={GAUGE_BASE.disk} size="lg" />
      </div>
      {summary.error && summary.status === 'ready' && (
        <p className="muted cluster-resource-soft-err" title={summary.error}>
          <Icon name="alert" size={12} />
          {summary.error}
        </p>
      )}
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
