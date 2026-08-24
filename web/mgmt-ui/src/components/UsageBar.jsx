function usageBarWidth(percent) {
  if (percent <= 0) return 0
  return Math.max(percent, 6)
}

function toPercent(value) {
  if (value == null || Number.isNaN(Number(value))) return 0
  return Math.max(0, Math.min(100, Number(value)))
}

export default function UsageBar({ metric, color = 'cpu' }) {
  const used = metric?.display_used
  const total = metric?.display_total
  const label = used && total ? `${used} / ${total}` : total || used || '—'
  const hasMetrics = metric?.percent != null && Number.isFinite(Number(metric.percent))
  const percent = toPercent(metric?.percent)

  return (
    <div className="usage-bar" title={metric?.error || label}>
      <span className="usage-bar-label">{label}</span>
      {hasMetrics ? (
        <>
          <div className="usage-bar-track">
            <div
              className={`usage-bar-fill usage-bar-${color}`}
              style={{ width: `${usageBarWidth(percent)}%` }}
            />
          </div>
          <span className="usage-bar-pct">{Math.round(percent)}%</span>
        </>
      ) : (
        <span className="usage-bar-empty">—</span>
      )}
    </div>
  )
}
