import { useEffect } from 'react'
import { Icon } from './Icons'

/** Form / content modal (distinct from confirm). */
export default function Modal({
  open,
  title,
  onClose,
  children,
  wide = false,
  icon = 'edit',
  tone = 'primary',
}) {
  useEffect(() => {
    if (!open) return
    function onKey(e) {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open, onClose])

  if (!open) return null

  return (
    <div className="modal-backdrop" role="presentation" onClick={onClose}>
      <div
        className={`modal-card ${wide ? 'modal-wide' : ''}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby="modal-title"
        onClick={(e) => e.stopPropagation()}
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
