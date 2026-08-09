/** Shared chips for guest arch + hypervisor kind. */

export function formatProviderKind(kind) {
  const k = String(kind || '').toLowerCase()
  if (k === 'vsphere' || k === 'esxi' || k === 'vmware') return 'vSphere'
  if (k === 'proxmox' || k === '') return 'Proxmox'
  return kind
}

export function normalizeProviderKind(kind) {
  const k = String(kind || '').toLowerCase()
  if (k === 'vsphere' || k === 'esxi' || k === 'vmware') return 'vsphere'
  if (k === 'proxmox' || k === '') return 'proxmox'
  return k || 'proxmox'
}

export function formatArch(arch) {
  const a = String(arch || 'amd64').toLowerCase()
  if (a === 'aarch64' || a === 'arm64') return 'arm64'
  return 'amd64'
}

/** Compact badges: arch + provider type (optional). */
export function ClusterMetaBadges({ arch, providerKind, className = '' }) {
  const a = formatArch(arch)
  const kind = normalizeProviderKind(providerKind)
  const kindLabel = formatProviderKind(kind)
  return (
    <span className={`cluster-meta-badges ${className}`.trim()}>
      <span className={`badge arch arch-${a}`} title={`Guest arch ${a}`}>
        {a}
      </span>
      <span className={`badge kind kind-${kind}`} title={`Provider ${kindLabel}`}>
        {kindLabel}
      </span>
    </span>
  )
}
