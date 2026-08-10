/** Lifecycle status + live Machine API online/offline badges. */
export function NodeStatusBadges({ status, availability }) {
  const life = status || 'unknown'
  const avail = availability || 'unknown'
  const showAvail = !!availability && availability !== 'unknown'
  return (
    <span className="status-badges">
      <span className={`badge ${life}`}>{life}</span>
      {showAvail && (
        <span className={`badge ${avail}`} title={availTitle(avail)}>
          {avail}
        </span>
      )}
    </span>
  )
}

function availTitle(a) {
  if (a === 'online') return 'Machine API (:50000) reachable'
  if (a === 'offline') return 'Machine API unreachable (powered off / wrong IP?)'
  return 'Availability unknown'
}
