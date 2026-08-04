/** Modern checkbox used in node multi-select tables. */
export default function Checkbox({ checked, indeterminate = false, onChange, label, id }) {
  return (
    <label className={`chk ${checked ? 'on' : ''}${indeterminate ? ' mid' : ''}`} htmlFor={id}>
      <input
        id={id}
        type="checkbox"
        checked={!!checked}
        ref={(el) => {
          if (el) el.indeterminate = !!indeterminate
        }}
        onChange={(e) => onChange?.(e.target.checked)}
      />
      <span className="chk-box" aria-hidden>
        <svg viewBox="0 0 16 16" width="12" height="12">
          {indeterminate ? (
            <path d="M3.5 8h9" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
          ) : (
            <path d="M3.5 8.5 6.5 11.5 12.5 4.5" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
          )}
        </svg>
      </span>
      {label ? <span className="chk-label">{label}</span> : null}
    </label>
  )
}
