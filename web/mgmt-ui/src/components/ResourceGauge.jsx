import { useMemo } from 'react'
import { Cell, Pie, PieChart, ResponsiveContainer } from 'recharts'
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

function gaugeFill(base, percent, hasPct) {
  if (!hasPct) return GAUGE_BASE.track
  if (percent >= 90) return 'var(--danger)'
  if (percent >= 75) return 'var(--warning)'
  return base
}

function gaugeData(percent) {
  const p = isNum(percent) ? Math.min(100, Math.max(0, percent)) : 0
  return [
    { name: 'used', value: p },
    { name: 'free', value: Math.max(0, 100 - p) },
  ]
}

function formatFallback(v, unit) {
  if (!isNum(v)) return '—'
  if (unit === 'cores') return v < 10 ? v.toFixed(2) : v.toFixed(1)
  if (unit === 'GiB') return `${v.toFixed(1)} GiB`
  return String(v)
}

export default function ResourceGauge({ label, icon, metric, color }) {
  const pct = metric?.percent
  const hasPct = isNum(pct)
  const data = useMemo(() => gaugeData(hasPct ? pct : 0), [hasPct, pct])
  const fill = gaugeFill(color, pct, hasPct)
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
    <div className={`resource-gauge resource-gauge-${level}`}>
      <div className="resource-gauge-label">
        {icon && <Icon name={icon} size={12} />}
        {label}
      </div>
      <div className="resource-gauge-chart">
        <ResponsiveContainer width="100%" height={88}>
          <PieChart>
            <Pie
              data={data}
              dataKey="value"
              cx="50%"
              cy="50%"
              startAngle={90}
              endAngle={-270}
              innerRadius={28}
              outerRadius={36}
              paddingAngle={hasPct && pct > 0 && pct < 100 ? 2 : 0}
              cornerRadius={4}
              stroke="none"
              isAnimationActive={false}
            >
              <Cell fill={fill} />
              <Cell fill={GAUGE_BASE.track} />
            </Pie>
          </PieChart>
        </ResponsiveContainer>
        <div className="resource-gauge-center">
          <span className="resource-gauge-pct">
            {hasPct ? `${Math.round(pct)}%` : '—'}
          </span>
        </div>
      </div>
      <div className="resource-gauge-breakdown">
        <div><span className="mono-inline">{usedLabel}</span> <span className="muted">used</span></div>
        <div><span className="mono-inline">{availLabel}</span> <span className="muted">avail</span></div>
        <div><span className="mono-inline">{totalLabel}</span> <span className="muted">total</span></div>
      </div>
      {metric?.error && (
        <div className="muted resource-gauge-err" title={metric.error}>{metric.error}</div>
      )}
    </div>
  )
}
