import { Icon } from './Icons'

export default function EmptyState({ icon = 'apps', title, message, action }) {
  return (
    <div className="empty-state-rich">
      <div className="empty-state-icon" aria-hidden>
        <Icon name={icon} size={22} />
      </div>
      {title && <h3 className="empty-state-title">{title}</h3>}
      {message && <p className="muted empty-state-msg">{message}</p>}
      {action}
    </div>
  )
}
