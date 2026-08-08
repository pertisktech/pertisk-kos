/** Lifecycle status + live online/offline badges. */
export function ClusterStatusBadges({ status, availability }) {
  const life = status || 'unknown'
  const avail = availability || 'unknown'
  return (
    <span className="status-badges">
      <span className={`badge ${life}`}>{life}</span>
      {life === 'ready' && (
        <span className={`badge ${avail}`} title={availTitle(avail)}>
          {avail}
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
