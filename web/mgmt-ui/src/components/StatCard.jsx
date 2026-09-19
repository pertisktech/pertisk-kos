import { Icon } from './Icons'

export default function StatCard({ label, value, hint, icon }) {
  return (
    <div className="stat-card">
      {icon ? (
        <span className="stat-card-icon" aria-hidden>
          <Icon name={icon} size={16} />
        </span>
      ) : null}
      <div className="stat-card-body">
        <p className="stat-card-label">{label}</p>
        <div className="stat-card-value">{value}</div>
        {hint ? <p className="stat-card-hint">{hint}</p> : null}
      </div>
    </div>
  )
}
