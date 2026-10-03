import { Icon } from './Icons'

export default function StatCard({ icon, label, value, hint, hintTone, valueTone }) {
  return (
    <div className="stat-card">
      {icon ? (
        <span className="stat-card-icon" aria-hidden>
          <Icon name={icon} size={16} />
        </span>
      ) : null}
      <div className="stat-card-body">
        <p className="stat-card-label">{label}</p>
        <div className={`stat-card-value${valueTone === 'warn' ? ' warn' : ''}`}>{value}</div>
        {hint ? (
          <p className={`stat-card-hint${hintTone === 'ok' ? ' ok' : ''}`}>{hint}</p>
        ) : null}
      </div>
    </div>
  )
}
