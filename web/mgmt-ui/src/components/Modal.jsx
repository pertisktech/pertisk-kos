import { Icon } from './Icons'

/** Form / content modal (distinct from confirm). Closes only via Close button. */
export default function Modal({
  open,
  title,
  onClose,
  children,
  wide = false,
  icon = 'edit',
  tone = 'primary',
  cardClassName = '',
}) {
  if (!open) return null

  return (
    <div className="modal-backdrop" role="presentation">
      <div
        className={`modal-card ${wide ? 'modal-wide' : ''} ${cardClassName}`.trim()}
        role="dialog"
        aria-modal="true"
        aria-labelledby="modal-title"
      >
        <div className="modal-head">
          <div className={`modal-icon ${tone}`}>
            <Icon name={icon} size={22} />
          </div>
          <button type="button" className="modal-close secondary btn-icon" onClick={onClose} aria-label="Close">
            <Icon name="x" size={16} />
          </button>
        </div>
        <h2 id="modal-title">{title}</h2>
        <div className="modal-body">{children}</div>
      </div>
    </div>
  )
}
