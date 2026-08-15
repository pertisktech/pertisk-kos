/** Live hypervisor API online/offline badge. */
export function ProviderStatusBadge({ availability, className = '', showUnknown = false }) {
  const avail = availability || 'unknown'
  if (!avail || avail === 'unknown') {
    if (!showUnknown) return null
    return (
      <span className={`badge unknown ${className}`.trim()} title="Availability unknown">
        unknown
      </span>
    )
  }
  return (
    <span className={`badge ${avail} ${className}`.trim()} title={availTitle(avail)}>
      {avail}
    </span>
  )
}

function availTitle(a) {
  if (a === 'online') return 'Hypervisor API reachable'
  if (a === 'offline') return 'Hypervisor API unreachable (host down / bad URL / credentials?)'
  return 'Availability unknown'
}
