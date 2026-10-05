import { Icon } from './Icons'

export default function StatCard({ icon, label, value, hint, hintTone, valueTone }) {
  const deltaTone = hintTone === 'ok' ? 'ok' : valueTone === 'warn' || hintTone === 'warn' ? 'warn' : null

  return (
    <div className="stat-card">
      <div className="stat-card-top">
        {icon ? (
          <span className="stat-card-icon" aria-hidden>
            <Icon name={icon} size={20} />
          </span>
        ) : (
          <span />
        )}
        {hint && deltaTone ? (
          <span className={`stat-card-delta ${deltaTone}`}>{hint}</span>
        ) : null}
      </div>
      <div className="stat-card-body">
        <p className="stat-card-label">{label}</p>
        <div className={`stat-card-value${valueTone === 'warn' ? ' warn' : ''}`}>{value}</div>
        {hint && !deltaTone ? <p className="stat-card-hint">{hint}</p> : null}
      </div>
    </div>
  )
}
