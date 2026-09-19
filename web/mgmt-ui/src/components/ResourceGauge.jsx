import { Icon } from './Icons'

export const GAUGE_BASE = {
  cpu: 'var(--resource-cpu)',
  memory: 'var(--resource-memory)',
  disk: 'var(--resource-disk)',
  track: 'color-mix(in srgb, var(--border) 70%, transparent)',
}

function isNum(v) {
  return typeof v === 'number' && Number.isFinite(v)
}

function barFill(base, percent, hasPct) {
  if (!hasPct) return GAUGE_BASE.track
  if (percent >= 85) return 'var(--danger)'
  if (percent >= 65) return 'var(--warning)'
  return base
}

function formatFallback(v, unit) {
  if (!isNum(v)) return '—'
  if (unit === 'cores' || unit === 'vCPU') return v < 10 ? v.toFixed(1) : String(Math.round(v))
  if (unit === 'GiB') return v.toFixed(1)
  return String(v)
}

function barWidth(percent) {
  if (percent <= 0) return 0
  return Math.max(percent, 4)
}

export default function ResourceGauge({ label, icon, metric, color, size = 'md', layout = 'tile' }) {
  const pct = metric?.percent
  const hasPct = isNum(pct)
  const fill = barFill(color, pct, hasPct)
  const rawUnit = metric?.unit || ''
  const unit = rawUnit === 'cores' ? 'vCPU' : rawUnit
  const used = metric?.used
  const total = metric?.total
  const avail = isNum(metric?.available)
    ? metric.available
    : isNum(used) && isNum(total)
      ? Math.max(0, total - used)
      : null
  const usedLabel = metric?.display_used || formatFallback(used, rawUnit || unit)
  const availLabel = metric?.display_available || formatFallback(avail, rawUnit || unit)
  const totalLabel = metric?.display_total || formatFallback(total, rawUnit || unit)
  const level = !hasPct ? 'unknown' : pct >= 85 ? 'critical' : pct >= 65 ? 'warn' : 'ok'
  const row = layout === 'row'
  const ratio = isNum(used) && isNum(total)
    ? `${formatFallback(used, rawUnit || unit)} / ${formatFallback(total, rawUnit || unit)}${unit ? ` ${unit}` : ''}`
    : hasPct
      ? `${Math.round(pct)}%`
      : '—'

  return (
    <div className={`metric-tile metric-tile-${level} metric-tile-${size}${row ? ' metric-tile-row' : ''}`}>
      <div className="metric-tile-top">
        <span className="metric-tile-label">
          {icon && <Icon name={icon} size={size === 'lg' ? 14 : 12} />}
          {label}
        </span>
        <span className="metric-tile-pct">
          {row ? ratio : hasPct ? `${Math.round(pct)}%` : '—'}
        </span>
      </div>
      {!row && <div className="metric-tile-headline">{usedLabel}</div>}
      <div className="metric-tile-track" aria-hidden>
        <div
          className="metric-tile-fill"
          style={{ width: `${hasPct ? barWidth(pct) : 0}%`, background: fill }}
        />
      </div>
      {!row && (
        <div className="metric-tile-stats">
          <span><em>{usedLabel}</em> used</span>
          <span><em>{availLabel}</em> free</span>
          <span><em>{totalLabel}</em> total</span>
        </div>
      )}
      {metric?.error && (
        <div className="muted metric-tile-err" title={metric.error}>{metric.error}</div>
      )}
    </div>
  )
}
