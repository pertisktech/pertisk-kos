export default function StatCard({ label, value, hint, hintTone, valueTone }) {
  return (
    <div className="stat-card">
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
