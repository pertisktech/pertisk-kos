import { Icon } from './Icons'

/**
 * Multi-step wizard dialog.
 * steps: [{ id, label }]
 * footer: React node (Back / Next / Save) — rendered below body
 * Closes only via Close / Cancel buttons (no backdrop or Escape).
 */
export default function WizardModal({
  open,
  title,
  onClose,
  steps = [],
  stepIndex = 0,
  onStepChange,
  children,
  footer,
  icon = 'plus',
  tone = 'primary',
}) {
  if (!open) return null

  return (
    <div className="modal-backdrop wizard-backdrop" role="presentation">
      <div
        className="modal-card modal-wizard"
        role="dialog"
        aria-modal="true"
        aria-labelledby="wizard-title"
      >
        <div className="modal-head">
          <div className={`modal-icon ${tone}`}>
            <Icon name={icon} size={22} />
          </div>
          <button type="button" className="modal-close secondary btn-icon" onClick={onClose} aria-label="Close">
            <Icon name="x" size={16} />
          </button>
        </div>
        <h2 id="wizard-title">{title}</h2>

        {steps.length > 0 && (
          <nav className="wizard-steps" aria-label="Wizard steps">
            {steps.map((s, i) => {
              const state =
                i === stepIndex ? 'current' : i < stepIndex ? 'done' : 'todo'
              return (
                <button
                  key={s.id}
                  type="button"
                  className={`wizard-step ${state}`}
                  onClick={() => {
                    if (i <= stepIndex && onStepChange) onStepChange(i)
                  }}
                  disabled={i > stepIndex}
                  aria-current={i === stepIndex ? 'step' : undefined}
                >
                  <span className="wizard-step-num">
                    {i < stepIndex ? <Icon name="check" size={12} /> : i + 1}
                  </span>
                  <span className="wizard-step-label">{s.label}</span>
                </button>
              )
            })}
          </nav>
        )}

        <div className="modal-body wizard-body">{children}</div>
        {footer && <div className="wizard-footer">{footer}</div>}
      </div>
    </div>
  )
}
