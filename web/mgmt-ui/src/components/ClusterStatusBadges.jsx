function titleCase(s) {
  if (!s) return 'Unknown'
  return s.charAt(0).toUpperCase() + s.slice(1)
}

/** Lifecycle status + live online/offline badges. */
export function ClusterStatusBadges({ status, availability }) {
  const life = status || 'unknown'
  const avail = availability || 'unknown'
  return (
    <span className="status-badges">
      <span className={`badge ${life}`}>{titleCase(life)}</span>
      {life === 'ready' && (
        <span className={`badge ${avail}`} title={availTitle(avail)}>
          {titleCase(avail)}
        </span>
      )}
    </span>
  )
}

/**
 * Single-column fleet status: prefer live reachability when lifecycle is ready.
 * Ready + offline must not render as healthy/online.
 */
export function clusterFleetStatus(status, availability) {
  const life = status || 'unknown'
  if (life === 'ready') {
    const avail = availability === 'online' || availability === 'offline' ? availability : 'unknown'
    if (avail === 'online') return { label: 'online', tone: 'ok' }
    if (avail === 'offline') return { label: 'offline', tone: 'err' }
    return { label: 'ready', tone: 'warn' }
  }
  if (life === 'error' || life === 'failed' || life === 'degraded') {
    return { label: life, tone: 'err' }
  }
  if (life === 'provisioning' || life === 'updating' || life === 'deleting') {
    return { label: life, tone: 'warn' }
  }
  return { label: life, tone: 'warn' }
}

/** Prefer offline over online when sources disagree (stale cache vs live probe). */
export function resolveAvailability(...vals) {
  if (vals.some((v) => v === 'offline')) return 'offline'
  if (vals.some((v) => v === 'online')) return 'online'
  for (const v of vals) {
    if (v) return v
  }
  return 'unknown'
}

function availTitle(a) {
  if (a === 'online') return 'Kubernetes API reachable'
  if (a === 'offline') return 'Kubernetes API unreachable (VMs powered off?)'
  return 'Availability unknown'
}
