import { Icon } from './Icons'

/**
 * Pill tab list. Arrow keys, Home, and End move the selection and focus.
 */
export default function TabBar({
  label,
  tabs,
  value,
  onChange,
  idPrefix,
  counts,
}) {
  function onKeyDown(e) {
    const keys = ['ArrowRight', 'ArrowLeft', 'Home', 'End']
    if (!keys.includes(e.key) || tabs.length === 0) return
    e.preventDefault()
    const ids = tabs.map((t) => t.id)
    const i = Math.max(0, ids.indexOf(value))
    let next = ids[i]
    if (e.key === 'ArrowRight') next = ids[(i + 1) % ids.length]
    if (e.key === 'ArrowLeft') next = ids[(i - 1 + ids.length) % ids.length]
    if (e.key === 'Home') next = ids[0]
    if (e.key === 'End') next = ids[ids.length - 1]
    onChange(next)
    requestAnimationFrame(() => {
      document.getElementById(`${idPrefix}-${next}`)?.focus()
    })
  }

  return (
    <div className="tabs" role="tablist" aria-label={label} onKeyDown={onKeyDown}>
      {tabs.map((t) => {
        const selected = value === t.id
        const count = counts?.[t.id]
        return (
          <button
            key={t.id}
            id={`${idPrefix}-${t.id}`}
            type="button"
            role="tab"
            aria-selected={selected}
            aria-controls={`${idPrefix}-panel`}
            tabIndex={selected ? 0 : -1}
            className={`tab${selected ? ' active' : ''}`}
            onClick={() => onChange(t.id)}
          >
            {t.icon ? <Icon name={t.icon} size={16} /> : null}
            {t.label}
            {count > 0 ? <span className="tab-count">{count}</span> : null}
          </button>
        )
      })}
    </div>
  )
}
