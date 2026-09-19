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

function availTitle(a) {
  if (a === 'online') return 'Kubernetes API reachable'
  if (a === 'offline') return 'Kubernetes API unreachable (VMs powered off?)'
  return 'Availability unknown'
}
