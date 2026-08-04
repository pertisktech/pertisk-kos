import { useEffect, useId, useRef, useState } from 'react'
import { api } from '../api'

/**
 * Modern K8s version dropdown — latest 10 stable releases from the API.
 */
export default function K8sVersionSelect({
  value,
  onChange,
  preferImage = true,
  disabled = false,
}) {
  const listId = useId()
  const rootRef = useRef(null)
  const [open, setOpen] = useState(false)
  const [loading, setLoading] = useState(true)
  const [versions, setVersions] = useState([])
  const [latest, setLatest] = useState('')
  const [image, setImage] = useState(null)
  const [loadError, setLoadError] = useState('')
  const seeded = useRef(false)

  useEffect(() => {
    let cancelled = false
    setLoading(true)
    api('/meta/k8s-versions')
      .then((res) => {
        if (cancelled) return
        let list = [...(res.versions || [])]
        const lat = res.latest || list[0] || ''
        const img = res.image || null
        setLatest(lat)
        setImage(img)
        setLoadError('')

        if (value && !list.includes(value)) {
          list = [value, ...list].slice(0, 11)
        }
        setVersions(list)

        if (!seeded.current) {
          seeded.current = true
          if (!value) {
            const preferred =
              (preferImage && img && list.includes(img) && img) || lat || list[0]
            if (preferred) onChange(preferred)
          }
        }
      })
      .catch((e) => {
        if (!cancelled) setLoadError(e.message || 'Failed to load versions')
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  useEffect(() => {
    if (!open) return
    function onDoc(e) {
      if (rootRef.current && !rootRef.current.contains(e.target)) setOpen(false)
    }
    function onKey(e) {
      if (e.key === 'Escape') setOpen(false)
    }
    document.addEventListener('mousedown', onDoc)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDoc)
      document.removeEventListener('keydown', onKey)
    }
  }, [open])

  const selected = value || latest || '—'

  return (
    <div className={`k8s-select ${open ? 'open' : ''}`} ref={rootRef}>
      <button
        type="button"
        className="k8s-select-trigger"
        disabled={disabled || (loading && !versions.length)}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={listId}
        onClick={() => setOpen((o) => !o)}
      >
        <span className="k8s-select-value mono-inline">
          {loading && !versions.length ? 'Loading…' : selected}
        </span>
        <span className="k8s-select-meta">
          {image && selected === image && <span className="k8s-pill image">image</span>}
          {latest && selected === latest && <span className="k8s-pill latest">latest</span>}
          <span className="k8s-chevron" aria-hidden />
        </span>
      </button>

      {open && (
        <ul id={listId} className="k8s-select-menu" role="listbox" aria-label="Kubernetes versions">
          {loading && versions.length === 0 && (
            <li className="k8s-select-empty">Loading versions…</li>
          )}
          {!loading && versions.length === 0 && (
            <li className="k8s-select-empty">{loadError || 'No versions available'}</li>
          )}
          {versions.map((ver) => {
            const isSel = ver === selected
            const isLatest = ver === latest
            const isImage = ver === image
            return (
              <li key={ver} role="option" aria-selected={isSel}>
                <button
                  type="button"
                  className={`k8s-select-option ${isSel ? 'selected' : ''}`}
                  onClick={() => {
                    onChange(ver)
                    setOpen(false)
                  }}
                >
                  <span className="mono-inline">{ver}</span>
                  <span className="k8s-select-option-tags">
                    {isLatest && <span className="k8s-pill latest">latest</span>}
                    {isImage && <span className="k8s-pill image">in image</span>}
                  </span>
                </button>
              </li>
            )
          })}
        </ul>
      )}
    </div>
  )
}
