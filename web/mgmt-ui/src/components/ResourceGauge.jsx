import { Icon } from './Icons'

export const GAUGE_BASE = {
  cpu: 'var(--color-blue-b1)',
  memory: '#a855f7',
  disk: '#10b981',
  track: 'color-mix(in srgb, var(--border) 70%, transparent)',
}

function isNum(v) {
  return typeof v === 'number' && Number.isFinite(v)
}

function barFill(base, percent, hasPct) {
  if (!hasPct) return GAUGE_BASE.track
  if (percent >= 90) return 'var(--danger)'
  if (percent >= 75) return 'var(--warning)'
  return base
}

function formatFallback(v, unit) {
  if (!isNum(v)) return '—'
  if (unit === 'cores') return v < 10 ? v.toFixed(2) : v.toFixed(1)
  if (unit === 'GiB') return `${v.toFixed(1)} GiB`
  return String(v)
}

function barWidth(percent) {
  if (percent <= 0) return 0
  return Math.max(percent, 4)
}

export default function ResourceGauge({ label, icon, metric, color, size = 'md' }) {
  const pct = metric?.percent
  const hasPct = isNum(pct)
  const fill = barFill(color, pct, hasPct)
  const unit = metric?.unit || ''
  const used = metric?.used
  const total = metric?.total
  const avail = isNum(metric?.available)
    ? metric.available
    : isNum(used) && isNum(total)
      ? Math.max(0, total - used)
      : null
  const usedLabel = metric?.display_used || formatFallback(used, unit)
  const availLabel = metric?.display_available || formatFallback(avail, unit)
  const totalLabel = metric?.display_total || formatFallback(total, unit)
  const level = !hasPct ? 'unknown' : pct >= 90 ? 'critical' : pct >= 75 ? 'warn' : 'ok'

  return (
    <div className={`metric-tile metric-tile-${level} metric-tile-${size}`}>
      <div className="metric-tile-top">
        <span className="metric-tile-label">
          {icon && <Icon name={icon} size={size === 'lg' ? 14 : 12} />}
          {label}
        </span>
        <span className="metric-tile-pct">
          {hasPct ? `${Math.round(pct)}%` : '—'}
        </span>
      </div>
      <div className="metric-tile-headline">{usedLabel}</div>
      <div className="metric-tile-track" aria-hidden>
        <div
          className="metric-tile-fill"
          style={{ width: `${hasPct ? barWidth(pct) : 0}%`, background: fill }}
        />
      </div>
      <div className="metric-tile-stats">
        <span><em>{usedLabel}</em> used</span>
        <span><em>{availLabel}</em> free</span>
        <span><em>{totalLabel}</em> total</span>
      </div>
      {metric?.error && (
        <div className="muted metric-tile-err" title={metric.error}>{metric.error}</div>
      )}
    </div>
  )
}
