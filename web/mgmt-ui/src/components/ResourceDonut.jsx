import { Icon } from './Icons'
import { GAUGE_BASE } from './ResourceGauge'

function isNum(v) {
  return typeof v === 'number' && Number.isFinite(v)
}

function formatFallback(v, unit) {
  if (!isNum(v)) return '—'
  if (unit === 'cores' || unit === 'vCPU') return v < 10 ? v.toFixed(1) : String(Math.round(v))
  if (unit === 'GiB') return v.toFixed(1)
  return String(v)
}

function fillColor(kind, pct, hasPct) {
  if (!hasPct) return 'var(--muted)'
  if (pct >= 85) return 'var(--danger)'
  if (pct >= 65) return 'var(--warning)'
  if (kind === 'cpu') return GAUGE_BASE.cpu
  if (kind === 'memory') return GAUGE_BASE.memory
  return GAUGE_BASE.disk
}

export default function ResourceDonut({
  kind = 'cpu',
  icon,
  label,
  metric,
  size = 64,
}) {
  const pct = metric?.percent
  const hasPct = isNum(pct)
  const value = hasPct ? Math.max(0, Math.min(100, Math.round(pct))) : 0
  const color = fillColor(kind, value, hasPct)
  const rawUnit = metric?.unit || ''
  const unit = rawUnit === 'cores' ? 'vCPU' : rawUnit
  const used = metric?.display_used || formatFallback(metric?.used, rawUnit || unit)
  const total = metric?.display_total || formatFallback(metric?.total, rawUnit || unit)
  const stroke = 6
  const r = (size - stroke) / 2
  const c = 2 * Math.PI * r
  const offset = c - (value / 100) * c

  return (
    <div className="resource-donut">
      <div className="resource-donut-ring" style={{ width: size, height: size }}>
        <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`} aria-hidden>
          <circle
            cx={size / 2}
            cy={size / 2}
            r={r}
            fill="none"
            stroke="var(--muted)"
            strokeWidth={stroke}
          />
          {hasPct ? (
            <circle
              cx={size / 2}
              cy={size / 2}
              r={r}
              fill="none"
              stroke={color}
              strokeWidth={stroke}
              strokeLinecap="round"
              strokeDasharray={c}
              strokeDashoffset={offset}
              transform={`rotate(-90 ${size / 2} ${size / 2})`}
            />
          ) : null}
        </svg>
        <div className="resource-donut-center">
          {icon ? <Icon name={icon} size={12} /> : null}
          <span className="resource-donut-pct">{hasPct ? `${value}%` : '—'}</span>
        </div>
      </div>
      <div className="resource-donut-copy">
        <p className="resource-donut-label">{label}</p>
        <p className="resource-donut-ratio">
          {used}/{total}
          {unit ? <span> {unit}</span> : null}
        </p>
      </div>
    </div>
  )
}
