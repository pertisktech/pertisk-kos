/** Shared helpers for dashboard / cluster / provider fleet panels. */

export function formatMetric(m) {
  if (!m) return '—'
  const used = m.display_used ?? m.used
  const total = m.display_total ?? m.total
  if (used == null && total == null) return '—'
  if (typeof used === 'string' && typeof total === 'string') {
    const um = used.trim().match(/^([\d.]+)\s*(.*)$/)
    const tm = total.trim().match(/^([\d.]+)\s*(.*)$/)
    if (um && tm && um[2] && um[2] === tm[2]) {
      return `${um[1]} / ${tm[1]} ${tm[2]}`.trim()
    }
  }
  const unit = m.display_used == null && m.display_total == null && m.unit ? ` ${m.unit}` : ''
  if (total == null) return `${used}${unit}`
  if (used == null) return `— / ${total}${unit}`
  return `${used} / ${total}${unit}`
}

export function metricPercent(m) {
  if (!m) return null
  if (typeof m.percent === 'number' && Number.isFinite(m.percent)) {
    return Math.max(0, Math.min(100, Math.round(m.percent)))
  }
  const used = typeof m.used === 'number' ? m.used : Number(m.used)
  const total = typeof m.total === 'number' ? m.total : Number(m.total)
  if (Number.isFinite(used) && Number.isFinite(total) && total > 0) {
    return Math.max(0, Math.min(100, Math.round((used / total) * 100)))
  }
  return null
}

export function resourceBarTone(pct) {
  if (pct == null) return 'muted'
  if (pct >= 80) return 'err'
  if (pct >= 60) return 'warn'
  return 'ok'
}

export function ResourceBars({ cpu, memory, disk }) {
  const items = [
    { label: 'CPU', metric: cpu },
    { label: 'Mem', metric: memory },
    { label: 'Disk', metric: disk },
  ]
  return (
    <div className="resource-bars">
      {items.map(({ label, metric }) => {
        const pct = metricPercent(metric)
        const tone = resourceBarTone(pct)
        return (
          <div key={label} className="resource-bar" title={formatMetric(metric)}>
            <span className="resource-bar-label">{label}</span>
            <div className="resource-bar-track" aria-hidden>
              <div
                className={`resource-bar-fill ${tone}`}
                style={{ width: `${pct == null ? 0 : pct}%` }}
              />
            </div>
            <span className="resource-bar-pct">{pct == null ? '—' : `${pct}%`}</span>
          </div>
        )
      })}
    </div>
  )
}
