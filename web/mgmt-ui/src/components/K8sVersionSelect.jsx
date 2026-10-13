import { useEffect, useId, useRef, useState } from 'react'
import { api } from '../api'
import Checkbox from './Checkbox'

const STORAGE_KEY = 'pertisk_k8s_include_prerelease'

function readStoredInclude() {
  try {
    return localStorage.getItem(STORAGE_KEY) === '1'
  } catch {
    return false
  }
}

function writeStoredInclude(on) {
  try {
    localStorage.setItem(STORAGE_KEY, on ? '1' : '0')
  } catch {
    /* ignore quota / private mode */
  }
}

function looksPrerelease(tag) {
  return /-(alpha|beta|rc)\.\d+$/i.test(String(tag || ''))
}

/**
 * K8s version dropdown — stable by default; optional alpha/beta/RC for testing new releases.
 */
export default function K8sVersionSelect({
  value,
  onChange,
  preferImage = true,
  disabled = false,
}) {
  const listId = useId()
  const toggleId = useId()
  const rootRef = useRef(null)
  const [open, setOpen] = useState(false)
  const [includePrerelease, setIncludePrerelease] = useState(() => {
    if (looksPrerelease(value)) return true
    return readStoredInclude()
  })
  const [loading, setLoading] = useState(true)
  const [versions, setVersions] = useState([])
  const [channels, setChannels] = useState({})
  const [latest, setLatest] = useState('')
  const [image, setImage] = useState(null)
  const [loadError, setLoadError] = useState('')
  const seeded = useRef(false)

  useEffect(() => {
    if (looksPrerelease(value) && !includePrerelease) {
      setIncludePrerelease(true)
      writeStoredInclude(true)
    }
  }, [value, includePrerelease])

  useEffect(() => {
    let cancelled = false
    setLoading(true)
    const q = includePrerelease ? '?include_prerelease=true' : ''
    api(`/meta/k8s-versions${q}`)
      .then((res) => {
        if (cancelled) return
        let list = [...(res.versions || [])]
        const lat = res.latest || list.find((v) => !looksPrerelease(v)) || list[0] || ''
        const img = res.image || null
        const ch = { ...(res.channels || {}) }
        setLatest(lat)
        setImage(img)
        setLoadError('')

        if (value && !list.includes(value)) {
          list = [value, ...list].slice(0, includePrerelease ? 26 : 11)
          if (!ch[value]) {
            const m = String(value).match(/-(alpha|beta|rc)\./i)
            ch[value] = m ? m[1].toLowerCase() : 'stable'
          }
        }
        setChannels(ch)
        setVersions(list)

        if (!seeded.current) {
          seeded.current = true
          if (!value) {
            const preferred =
              (preferImage && img && list.includes(img) && img)
              || lat
              || list.find((v) => !looksPrerelease(v))
              || list[0]
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
  }, [includePrerelease])

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
  const selectedChannel = channels[selected]

  function togglePrerelease(next) {
    setIncludePrerelease(next)
    writeStoredInclude(next)
  }

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
          {selectedChannel && selectedChannel !== 'stable' && (
            <span className={`k8s-pill ${selectedChannel}`}>{selectedChannel}</span>
          )}
          {image && selected === image && <span className="k8s-pill image">image</span>}
          {latest && selected === latest && <span className="k8s-pill latest">latest</span>}
          <span className="k8s-chevron" aria-hidden />
        </span>
      </button>

      {open && (
        <div className="k8s-select-panel">
          <div className="k8s-select-filter">
            <Checkbox
              id={toggleId}
              checked={includePrerelease}
              onChange={(checked) => togglePrerelease(checked)}
              label="Include prereleases"
            />
          </div>
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
              const channel = channels[ver]
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
                      {channel && channel !== 'stable' && (
                        <span className={`k8s-pill ${channel}`}>{channel}</span>
                      )}
                      {isLatest && <span className="k8s-pill latest">latest</span>}
                      {isImage && <span className="k8s-pill image">in image</span>}
                    </span>
                  </button>
                </li>
              )
            })}
          </ul>
        </div>
      )}
    </div>
  )
}
