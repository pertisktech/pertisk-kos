import { useEffect } from 'react'
import { Icon } from './Icons'

/**
 * Slide-over panel (Vela-style). Esc / backdrop closes when `onClose` is set.
 */
export default function Drawer({
  open,
  onClose,
  title,
  subtitle,
  width = 'min(520px, 96vw)',
  children,
  footer,
}) {
  useEffect(() => {
    if (!open) return undefined
    function onKey(e) {
      if (e.key === 'Escape') onClose?.()
    }
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  }, [open, onClose])

  if (!open) return null

  return (
    <div className="drawer-root" role="presentation">
      <button type="button" className="drawer-backdrop" aria-label="Close" onClick={onClose} />
      <aside className="drawer-panel" style={{ width }} role="dialog" aria-modal="true" aria-label={title || 'Details'}>
        <header className="drawer-header">
          <div className="drawer-title-block">
            {title && <h2 className="drawer-title">{title}</h2>}
            {subtitle && <p className="drawer-subtitle muted">{subtitle}</p>}
          </div>
          {onClose && (
            <button type="button" className="theme-toggle" onClick={onClose} aria-label="Close drawer">
              <Icon name="x" size={16} />
            </button>
          )}
        </header>
        <div className="drawer-body">{children}</div>
        {footer && <footer className="drawer-footer">{footer}</footer>}
      </aside>
    </div>
  )
}
