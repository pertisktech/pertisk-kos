import { createContext, useCallback, useContext, useRef, useState } from 'react'
import { Icon } from './Icons'

const ConfirmCtx = createContext(null)

export function ConfirmProvider({ children }) {
  const [state, setState] = useState(null)
  const resolver = useRef(null)

  const confirm = useCallback((opts) => {
    return new Promise((resolve) => {
      resolver.current = resolve
      setState({
        title: opts.title || 'Confirm',
        message: opts.message || 'Are you sure?',
        confirmLabel: opts.confirmLabel || 'Confirm',
        cancelLabel: opts.cancelLabel || 'Cancel',
        tone: opts.tone || 'danger', // danger | primary
      })
    })
  }, [])

  function close(result) {
    setState(null)
    resolver.current?.(result)
    resolver.current = null
  }

  return (
    <ConfirmCtx.Provider value={confirm}>
      {children}
      {state && (
        <div className="modal-backdrop confirm-backdrop" role="presentation" onClick={() => close(false)}>
          <div
            className="modal-card"
            role="dialog"
            aria-modal="true"
            aria-labelledby="confirm-title"
            onClick={(e) => e.stopPropagation()}
          >
            <div className={`modal-icon ${state.tone}`}>
              <Icon name={state.tone === 'danger' ? 'alert' : 'check'} size={22} />
            </div>
            <h2 id="confirm-title">{state.title}</h2>
            <p className="muted" style={{ whiteSpace: 'pre-line' }}>{state.message}</p>
            <div className="modal-actions">
              <button type="button" className="secondary" onClick={() => close(false)}>
                {state.cancelLabel}
              </button>
              <button
                type="button"
                className={state.tone === 'danger' ? 'danger' : ''}
                onClick={() => close(true)}
                autoFocus
              >
                {state.confirmLabel}
              </button>
            </div>
          </div>
        </div>
      )}
    </ConfirmCtx.Provider>
  )
}

export function useConfirm() {
  const ctx = useContext(ConfirmCtx)
  if (!ctx) throw new Error('useConfirm requires ConfirmProvider')
  return ctx
}
